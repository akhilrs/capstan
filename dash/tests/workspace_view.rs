//! The workspace views of the dash: integration sets in the pipeline, per-repo verify, multi-repo reports and the
//! worktrees an agent holds. The frames are built from status JSON the way the daemon sends it (ws-feed).
use cstan_dash::model::{build_dash_model, DashModel};
use cstan_dash::view::{build_frame, cell_width, make_theme, ThemeOptions, ViewState};
use serde_json::{json, Value};

const NOW: i64 = 1_790_942_400_000;

fn iso(seconds_ago: i64) -> String {
    let ms = NOW - seconds_ago * 1000;
    let days = ms.div_euclid(86_400_000);
    let rest = ms.rem_euclid(86_400_000) / 1000;
    let day = 2 + (days - 20_728);
    format!(
        "2026-10-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

fn member(repo: &str, position: i64, state: &str, head: Option<&str>) -> Value {
    json!({
        "repo": repo, "position": position, "attempt": 0, "integrationId": format!("int-{repo}"),
        "state": state, "branch": "integration/plan-185", "headSha": head, "dropped": false,
    })
}

fn with(mut m: Value, key: &str, value: Value) -> Value {
    m[key] = value;
    m
}

fn verify(state: &str, sha: &str) -> Value {
    json!({"state": state, "commitSha": sha, "lastPassed": null})
}

fn set(id: &str, state: &str, members: Vec<Value>, gate: Value) -> Value {
    json!({"setId": id, "state": state, "branch": "integration/plan-185", "confirmed": false,
           "members": members, "gate": gate})
}

fn status(sets: Vec<Value>, reports: Vec<Value>) -> Value {
    let integrations: Vec<Value> = sets
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({"integrationId": format!("int-{}", s["setId"].as_str().unwrap()), "state": "merged",
                   "createdAt": iso(60 * (i as i64 + 1)),
                   "set": {"setId": s["setId"], "state": s["state"]}})
        })
        .collect();
    json!({
        "projectId": "p", "run": {"state": "running"},
        "agents": [], "reports": reports, "reviews": [], "integrations": integrations,
        "integrationSets": sets,
    })
}

fn model_of(status: &Value) -> DashModel {
    build_dash_model(status, NOW, None)
}

fn view(columns: u16, rows: u16, selected: usize) -> ViewState {
    let v: Value = json!({
        "size": {"columns": columns, "rows": rows}, "focus": "pipeline",
        "selected": {"agents": 0, "pipeline": selected, "queue": 0, "findings": 0, "work": 0},
        "problemsOnly": false, "showAllEnded": false, "paused": false, "link": "ok", "linkAge": "1s",
        "clock": "15:02:23", "intervalSeconds": 2, "nowMs": NOW, "tick": 0,
        "rings": {"unresolved": [], "working": [], "oldest": []},
        "highlight": [], "notice": null,
    });
    serde_json::from_value(v).expect("view")
}

fn frame_text(
    model: &DashModel,
    columns: u16,
    rows: u16,
    selected: usize,
    ascii: bool,
) -> Vec<String> {
    let theme = make_theme(ThemeOptions {
        no_color: true,
        reduced_motion: true,
        ascii,
    });
    let frame = build_frame(model, &view(columns, rows, selected), &theme);
    let frame = serde_json::to_value(&frame).unwrap();
    frame["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l.as_array()
                .unwrap()
                .iter()
                .map(|s| s["text"].as_str().unwrap())
                .collect()
        })
        .collect()
}

#[allow(dead_code)]
fn dump(lines: &[String]) -> String {
    lines.join("\n")
}

fn has(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

fn all_sets() -> Value {
    let running = set(
        "1c2d3e4f00000000",
        "running",
        vec![
            member("api", 1, "running", None),
            member("web", 2, "running", None),
        ],
        Value::Null,
    );
    let partly_built = set(
        "2a7f0b1900000000",
        "running",
        vec![
            member("api", 1, "merged", Some("9f3c2a1aa")),
            member("web", 2, "merged", Some("51ab07eaa")),
            member("worker", 3, "running", None),
            member("infra", 4, "running", None),
        ],
        Value::Null,
    );
    let conflicted = set(
        "3b81e2c400000000",
        "conflicted",
        vec![
            member("api", 1, "merged", Some("9f3c2a1aa")),
            member("web", 2, "conflicted", None),
        ],
        Value::Null,
    );
    let mut failed_member = member("worker", 3, "failed", None);
    failed_member["attempt"] = json!(1);
    let failed = set(
        "4c90a7d200000000",
        "failed",
        vec![
            member("api", 1, "merged", Some("9f3c2a1aa")),
            member("web", 2, "merged", Some("51ab07eaa")),
            failed_member,
        ],
        Value::Null,
    );
    let dropped = set(
        "8a46cad200000000",
        "merged",
        vec![
            with(
                member("api", 1, "confirmed", Some("9f3c2a1aa")),
                "verify",
                verify("passed", "9f3c2a1"),
            ),
            with(
                member("web", 2, "merged", Some("51ab07eaa")),
                "verify",
                verify("passed", "51ab07e"),
            ),
            with(
                member("worker", 3, "merged", Some("c04d9e2aa")),
                "verify",
                verify("running", "c04d9e2"),
            ),
            with(
                member("infra", 4, "discarded", None),
                "dropped",
                json!(true),
            ),
        ],
        json!({"state": "waiting", "repos": ["worker"]}),
    );
    let discarded = set(
        "5d13f6a800000000",
        "discarded",
        vec![
            member("api", 1, "discarded", None),
            member("web", 2, "discarded", None),
        ],
        Value::Null,
    );
    let all_built = set(
        "4b7e91c000000000",
        "merged",
        vec![
            member("api", 1, "merged", Some("9f3c2a1aa")),
            member("web", 2, "merged", Some("51ab07eaa")),
        ],
        Value::Null,
    );
    let all_confirmed = set(
        "7f35b9c100000000",
        "confirmed",
        vec![
            member("api", 1, "confirmed", Some("9f3c2a1aa")),
            member("web", 2, "confirmed", Some("51ab07eaa")),
        ],
        Value::Null,
    );
    json!([
        running,
        partly_built,
        conflicted,
        failed,
        dropped,
        discarded,
        all_built,
        all_confirmed
    ])
}

fn sets_of(v: &Value) -> Vec<Value> {
    v.as_array().unwrap().clone()
}

fn row_of<'a>(lines: &'a [String], set_id: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.contains(&set_id[..8]) && l.contains("set "))
        .unwrap_or_else(|| panic!("no row for {set_id}\n{}", lines.join("\n")))
}

#[test]
fn every_set_state_has_its_row() {
    let model = model_of(&status(sets_of(&all_sets()), vec![]));
    let lines = frame_text(&model, 120, 50, 0, false);
    for (id, strip, summary) in [
        ("1c2d3e4f", "◌◌", "building 0/2 built"),
        ("2a7f0b19", "○○◌◌", "building 2/4 built"),
        ("3b81e2c4", "○▲", "conflicted: web"),
        ("4c90a7d2", "○○✕", "failed: worker"),
        ("8a46cad2", "●○○–", "1/3 in main"),
        ("5d13f6a8", "––", "discarded"),
        ("4b7e91c0", "○○", "built 2/2"),
        ("7f35b9c1", "●●", "in main 2/2"),
    ] {
        let row = row_of(&lines, id);
        assert!(row.contains(strip), "{id} strip {strip}: {row}");
        assert!(row.contains(summary), "{id} summary {summary}: {row}");
    }
    // The members' own integration rows are folded into their set.
    assert!(!has(&lines, "int-"));
    assert_eq!(
        lines.iter().filter(|l| l.contains("integration ")).count(),
        0
    );
}

#[test]
fn the_stage_line_counts_a_set_once() {
    let model = model_of(&status(sets_of(&all_sets()), vec![]));
    assert_eq!(model.pipeline.integrations.total, 8);
    assert_eq!(model.pipeline.integrations.counts["building"], 2);
    assert_eq!(model.pipeline.integrations.counts["in main"], 1);
}

#[test]
fn a_selected_set_lists_its_members_in_merge_order_with_the_gate() {
    let model = model_of(&status(sets_of(&all_sets()), vec![]));
    let index = cstan_dash::model::pipeline_items(&model)
        .iter()
        .position(|i| i.id == "set:8a46cad200000000")
        .unwrap();
    let lines = frame_text(&model, 120, 50, index, false);
    assert!(has(
        &lines,
        "set 8a46cad2 integration/plan-185  1 of 3 in main"
    ));
    assert!(has(&lines, "REPO"));
    let find = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle}"))
    };
    let (api, web, worker, infra) = (
        find("● in main"),
        find("○ built   51ab07e"),
        find("○ built   c04d9e2"),
        find("– dropped"),
    );
    assert!(api < web && web < worker && worker < infra);
    assert!(lines[web].contains('▌') && lines[web].contains("next"));
    assert!(lines[api].contains("review") && lines[worker].contains("running"));
    assert!(lines[web].contains("passed"));
    assert!(lines[infra].contains("outside the gate"));
    assert!(has(&lines, "gate waiting: worker"));
}

#[test]
fn a_queued_member_run_reads_running() {
    let mut s = all_sets();
    s[4]["members"][2]["verify"] = verify("queued", "c04d9e2");
    let model = model_of(&status(sets_of(&s), vec![]));
    let set = model
        .pipeline
        .integrations
        .items
        .iter()
        .find_map(|i| i.set.as_ref().filter(|s| s.set_id.starts_with("8a46")))
        .unwrap();
    assert_eq!(set.members[2].verify.as_ref().unwrap().state, "running");
}

#[test]
fn a_redone_member_shows_its_attempt_and_a_failed_set_names_it() {
    let model = model_of(&status(sets_of(&all_sets()), vec![]));
    let index = cstan_dash::model::pipeline_items(&model)
        .iter()
        .position(|i| i.id == "set:4c90a7d200000000")
        .unwrap();
    let lines = frame_text(&model, 120, 50, index, false);
    assert!(has(&lines, "failed in worker"));
    assert!(has(&lines, "✕ failed"));
    assert!(has(&lines, "attempt 2"));
}

#[test]
fn a_set_of_32_repos_fits_the_width_rules_at_80_columns() {
    let members: Vec<Value> = (0..32)
        .map(|i| {
            let name = match i {
                5 => "payments-reconciliation-service".to_string(),
                _ => format!("repo-{i:02}"),
            };
            let state = if i < 5 { "confirmed" } else { "merged" };
            with(
                member(&name, i + 1, state, Some("3a1b9c0aa")),
                "verify",
                verify("passed", "3a1b9c0"),
            )
        })
        .collect();
    let s = set(
        "9a01c3d700000000",
        "merged",
        members,
        json!({"state": "passed", "repos": []}),
    );
    let model = model_of(&status(vec![s], vec![]));
    let lines = frame_text(&model, 80, 50, 0, false);
    for line in &lines {
        assert_eq!(cell_width(line), 80, "{line}");
    }
    let row = row_of(&lines, "9a01c3d7");
    assert!(row.contains("●●●●●○○+") || row.contains("+"), "{row}");
    assert!(row.contains("5/32 in main"));
    // The area scrolls around the next repo (the sixth) and says what is hidden.
    assert!(has(&lines, "payme…service"));
    assert!(has(
        &lines,
        "payme…service = payments-reconciliation-service"
    ));
    assert!(has(&lines, "below"));
}

#[test]
fn the_strip_follows_the_cap_rules() {
    use cstan_dash::view::panels::workspace::strip_window as window;
    assert_eq!(window(4, None, 10), (0, 4));
    assert_eq!(window(32, None, 10), (0, 7));
    let (start, end) = window(32, Some(20), 10);
    assert!(
        start > 0 && end - start <= 7 && start <= 18 && end > 20,
        "{start} {end}"
    );
}

#[test]
fn ascii_uses_letters() {
    let model = model_of(&status(sets_of(&all_sets()), vec![]));
    let lines = frame_text(&model, 120, 50, 0, true);
    let row = row_of(&lines, "3b81e2c4");
    assert!(row.contains("o!"), "{row}");
    let row = row_of(&lines, "8a46cad2");
    assert!(row.contains("#oo-"), "{row}");
}

#[test]
fn a_multi_repo_report_shows_its_count_and_expands_to_commits() {
    let report = json!({
        "reportId": "r-1", "agentId": "developer-4", "generation": 1, "state": "accepted",
        "createdAt": iso(30),
        "commits": [
            {"repo": "api", "commitSha": "9f3c2a1aaaaa", "branch": "feat/x", "baseSha": "0"},
            {"repo": "web", "commitSha": "51ab07eaaaaa", "branch": "feat/x", "baseSha": "0"},
            {"repo": "worker", "commitSha": "c04d9e2aaaaa", "branch": "feat/x", "baseSha": "0"},
        ],
    });
    let single = json!({
        "reportId": "r-2", "agentId": "developer-5", "generation": 1, "state": "accepted",
        "commitSha": "1234567aaaa", "createdAt": iso(90),
    });
    let model = model_of(&status(vec![], vec![report, single]));
    let lines = frame_text(&model, 120, 40, 0, false);
    assert!(has(&lines, "developer-4 3 repos 9f3c2a1"));
    assert!(has(
        &lines,
        "api 9f3c2a1 · web 51ab07e · worker c04d9e2  on feat/x"
    ));
    // A single-repo report keeps its label and opens nothing.
    assert!(has(&lines, "developer-5 1234567"));
    let lines = frame_text(&model, 120, 40, 1, false);
    assert!(!has(&lines, "selected"));
}

#[test]
fn the_agents_panel_shows_repos_and_the_worktrees() {
    let mut s = status(vec![], vec![]);
    s["agents"] = json!([
        {"agentId": "developer-4", "roleName": "developer", "kind": "Developer", "generation": 1,
         "state": "active", "lastActivityAt": iso(3),
         "worktrees": [
            {"repo": "api", "path": "/w/developer-4/api", "branch": "feat/PM-512"},
            {"repo": "web", "path": "/w/developer-4/web", "branch": "feat/PM-512"}],
         "changeFolder": "/w/developer-4"},
        {"agentId": "pm-1", "roleName": "pm", "kind": "PM", "generation": 1, "state": "active",
         "lastActivityAt": iso(80)},
    ]);
    let model = model_of(&s);
    let theme = make_theme(ThemeOptions {
        no_color: true,
        reduced_motion: true,
        ascii: false,
    });
    let mut v = view(120, 30, 0);
    v.focus = cstan_dash::view::PanelId::Agents;
    let frame = serde_json::to_value(build_frame(&model, &v, &theme)).unwrap();
    let lines: Vec<String> = frame["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l.as_array()
                .unwrap()
                .iter()
                .map(|s| s["text"].as_str().unwrap())
                .collect()
        })
        .collect();
    assert!(has(&lines, "REPOS"));
    assert!(has(&lines, "developer-4 worktrees on feat/PM-512"));
    assert!(has(&lines, "/w/developer-4/api"));
    assert!(has(&lines, "change folder"));
    // Narrow terminals leave REPOS out.
    let mut narrow = view(99, 30, 0);
    narrow.focus = cstan_dash::view::PanelId::Agents;
    let frame = serde_json::to_value(build_frame(&model, &narrow, &theme)).unwrap();
    assert!(!frame.to_string().contains("REPOS"));
}

#[test]
fn a_project_without_sets_is_unchanged() {
    let s = json!({"projectId": "p", "run": {"state": "running"}, "agents": [], "reports": [],
        "reviews": [], "integrations": [{"integrationId": "7c1e0f2a", "state": "merged", "createdAt": iso(60)}]});
    let model = model_of(&s);
    assert!(model
        .pipeline
        .integrations
        .items
        .iter()
        .all(|i| i.set.is_none()));
    let json = serde_json::to_string(&model).unwrap();
    assert!(!json.contains("\"set\"") && !json.contains("commits") && !json.contains("worktrees"));
}

#[test]
fn the_selected_area_scrolls_to_the_next_repo_and_counts_what_is_hidden() {
    let members: Vec<Value> = (0..32)
        .map(|i| {
            let state = if i < 25 { "confirmed" } else { "merged" };
            member(&format!("repo-{i:02}"), i + 1, state, Some("3a1b9c0aa"))
        })
        .collect();
    let s = set("9a01c3d700000000", "merged", members, Value::Null);
    let model = model_of(&status(vec![s], vec![]));
    let lines = frame_text(&model, 80, 34, 0, false);
    for line in &lines {
        assert_eq!(cell_width(line), 80, "{line}");
    }
    assert!(has(&lines, "▲ "), "{}", dump(&lines));
    let next = lines
        .iter()
        .find(|l| l.contains("▌26"))
        .expect("the next repo is in view");
    assert!(next.contains("repo-25") && next.contains("next"));
    let row = row_of(&lines, "9a01c3d7");
    assert!(row.contains("+"), "{row}");
}

fn set_fingerprint(sets: Vec<Value>) -> String {
    let m = model_of(&status(sets, vec![]));
    m.pipeline.integrations.items[0].fingerprint.clone()
}

fn base_set() -> Value {
    set(
        "9a9a9a9a00000000",
        "running",
        vec![
            with(
                member("api", 1, "merged", Some("aaaaaaa1")),
                "attempt",
                json!(0),
            ),
            member("web", 2, "running", None),
        ],
        json!({"state": "waiting", "repos": ["web"]}),
    )
}

#[test]
fn every_displayed_set_field_changes_the_row_fingerprint() {
    let base = set_fingerprint(vec![base_set()]);
    let changes: Vec<(&str, Value)> = vec![
        ("gate", {
            let mut s = base_set();
            s["gate"] = json!({"state": "passed", "repos": ["web"]});
            s
        }),
        ("gate repos", {
            let mut s = base_set();
            s["gate"] = json!({"state": "waiting", "repos": ["web", "api"]});
            s
        }),
        ("member verify", {
            let mut s = base_set();
            s["members"][0]["verify"] = verify("passed", "aaaaaaa1");
            s
        }),
        ("member head", {
            let mut s = base_set();
            s["members"][0]["headSha"] = json!("bbbbbbb2");
            s
        }),
        ("member attempt", {
            let mut s = base_set();
            s["members"][0]["attempt"] = json!(2);
            s
        }),
        ("member dropped", {
            let mut s = base_set();
            s["members"][1]["dropped"] = json!(true);
            s
        }),
        ("review subject", {
            let mut s = base_set();
            s["members"][0]["integrationId"] = json!(null);
            s
        }),
    ];
    for (what, changed) in changes {
        assert_ne!(set_fingerprint(vec![changed]), base, "{what}");
    }
}

#[test]
fn a_changed_set_field_highlights_the_row_on_the_next_poll() {
    let before = model_of(&status(vec![base_set()], vec![]));
    let mut s = base_set();
    s["members"][0]["verify"] = verify("failed", "aaaaaaa1");
    let after = build_dash_model(&status(vec![s], vec![]), NOW, None);
    let again = build_dash_model(&status(vec![base_set()], vec![]), NOW, None);
    assert_eq!(before.pipeline.integrations.items.len(), 1);
    assert_ne!(
        before.pipeline.integrations.items[0].fingerprint,
        after.pipeline.integrations.items[0].fingerprint
    );
    assert_eq!(
        before.pipeline.integrations.items[0].fingerprint,
        again.pipeline.integrations.items[0].fingerprint
    );
}

#[test]
fn the_integrations_stage_keeps_the_true_counts_with_sets() {
    let mut st = status(vec![base_set()], vec![]);
    // The set's member row, one more member row of nowhere, and 40 plain rows the status did not list.
    st["integrations"]
        .as_array_mut()
        .unwrap()
        .push(json!({"integrationId": "solo", "state": "merged", "createdAt": iso(5)}));
    st["pipelineCounts"] = json!({"integrations": {"merged": 42}});
    let m = model_of(&st);
    let stage = &m.pipeline.integrations;
    // 42 merged rows less the one folded member row, plus the set itself.
    assert_eq!(stage.total, 42);
    assert_eq!(stage.counts.get("merged"), Some(&41));
    assert!(!stage.capped);
}

#[test]
fn the_integrations_stage_keeps_the_capped_flag_with_sets() {
    let mut st = status(vec![base_set()], vec![]);
    for i in 0..25 {
        st["integrations"].as_array_mut().unwrap().push(
            json!({"integrationId": format!("s{i}"), "state": "merged", "createdAt": iso(1000 + i)}),
        );
    }
    let m = model_of(&st);
    assert!(m.pipeline.integrations.capped);
    assert_eq!(m.pipeline.integrations.items.len(), 20);
}

//! The details recorded once at merge: `GitRepo::diff_stat` over a real repository, `integration_impact::details`
//! with its caps, and the insert inside the finishing mutation (once, never rewritten, nothing for a finish that did not
//! merge). The merge path of `finish_merge` itself runs in the parity-integrate replays.

#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_config::{ImpactConfig, NextStep};
use capstan_kernel::areas::integrations::finish_integration_with_details;
use capstan_kernel::integrate::{DiffEntry, DiffStat, DiffStatError, GitRepo, DIFF_STAT_MAX_PATHS};
use capstan_kernel::integration_impact::{details, DETAILS_MAX_BYTES, TEXT_MAX_BYTES};
use fixture::{context, Fixture, OWNER};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

// --- a git fixture --------------------------------------------------------------------------------------------------

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .stdin(Stdio::null())
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn write(root: &Path, path: &str, bytes: &[u8]) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
}

fn commit(root: &Path, message: &str) -> String {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
    git(root, &["rev-parse", "HEAD"])
}

fn repo() -> (tempfile::TempDir, GitRepo) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    let repo = GitRepo::new(dir.path().to_path_buf());
    (dir, repo)
}

fn config() -> ImpactConfig {
    ImpactConfig {
        areas: vec!["crates/*".into(), "docs".into()],
        migrations: vec!["migrations/*.sql".into()],
        next_steps: vec![
            NextStep {
                paths: vec!["migrations/*.sql".into()],
                step: "back up the ledger".into(),
            },
            NextStep {
                paths: vec!["crates/**".into()],
                step: "rebuild".into(),
            },
            NextStep {
                paths: vec!["nothing/**".into()],
                step: "never".into(),
            },
        ],
    }
}

fn info(reports: usize) -> Value {
    json!({
        "planId": "plan-1",
        "planTitle": "A title",
        "packages": [],
        "reports": (1..=reports).map(|n| json!({
            "reportId": format!("r{n}"), "agentId": "dev-1", "summary": format!("summary {n}"), "branch": "b",
        })).collect::<Vec<_>>(),
    })
}

fn size(value: &Value) -> usize {
    capstan_kernel::json::stringify(value).len()
}

// --- diff_stat ------------------------------------------------------------------------------------------------------

#[test]
fn a_diff_lists_status_counts_and_binary_files() {
    let (dir, repo) = repo();
    let root = dir.path();
    write(root, "migrations/0001_a.sql", b"create table a(x);\n");
    write(root, "crates/kernel/src/lib.rs", b"one\ntwo\n");
    write(root, "docs/old.md", b"gone\n");
    let base = commit(root, "base");
    write(
        root,
        "migrations/0002_b.sql",
        b"create table b(x);\nselect 1;\n",
    );
    write(root, "migrations/0001_a.sql", b"create table a(x, y);\n");
    write(root, "crates/kernel/src/lib.rs", b"one\ntwo\nthree\n");
    write(root, "blob.bin", &[0, 1, 2, 3, 0, 255, 0, 7]);
    std::fs::remove_file(root.join("docs/old.md")).unwrap();
    let head = commit(root, "change");

    let stat = repo.diff_stat(&base, &head).unwrap();
    assert_eq!(stat.files_changed, 5);
    assert!(!stat.truncated);
    let by_path: HashMap<&str, &DiffEntry> =
        stat.entries.iter().map(|e| (e.path.as_str(), e)).collect();
    assert_eq!(by_path["migrations/0002_b.sql"].status, 'A');
    assert_eq!(by_path["migrations/0002_b.sql"].insertions, 2);
    assert_eq!(by_path["migrations/0001_a.sql"].status, 'M');
    assert_eq!(by_path["docs/old.md"].status, 'D');
    assert_eq!(by_path["docs/old.md"].deletions, 1);
    let binary = by_path["blob.bin"];
    assert_eq!(
        (binary.status, binary.insertions, binary.deletions),
        ('A', 0, 0)
    );
    assert_eq!(stat.insertions, 2 + 1 + 1);
    assert_eq!(stat.deletions, 1 + 1);

    let value = details(&info(2), &HashMap::new(), Ok(&stat), &config());
    let impact = &value["impact"];
    assert_eq!(impact["filesChanged"], 5);
    assert_eq!(impact["migrations"], json!(["migrations/0002_b.sql"]));
    assert_eq!(
        impact["nextSteps"],
        json!(["back up the ledger", "rebuild"])
    );
    assert_eq!(
        impact["areas"][0],
        json!({"path": "migrations", "files": 2})
    );
    assert_eq!(value["impactError"], Value::Null);
}

#[test]
fn many_paths_give_counts_and_a_truncated_flag() {
    let (dir, repo) = repo();
    let root = dir.path();
    write(root, "keep.txt", b"k\n");
    let base = commit(root, "base");
    for n in 0..6000 {
        write(root, &format!("bulk/f{n:05}.txt"), b"x\n");
    }
    let head = commit(root, "bulk");
    let stat = repo.diff_stat(&base, &head).unwrap();
    assert!(stat.truncated);
    assert_eq!(stat.files_changed, 6000);
    assert_eq!(stat.insertions, 6000);
    assert_eq!(stat.entries.len(), DIFF_STAT_MAX_PATHS);
    let value = details(
        &info(1),
        &HashMap::new(),
        Ok(&stat),
        &ImpactConfig::default(),
    );
    assert_eq!(value["impact"]["truncated"], true);
    assert_eq!(value["impact"]["filesChanged"], 6000);
}

#[test]
fn a_bad_commit_is_a_failed_diff() {
    let (dir, repo) = repo();
    write(dir.path(), "a", b"a\n");
    let base = commit(dir.path(), "base");
    assert_eq!(
        repo.diff_stat(&base, &"1".repeat(40)),
        Err(DiffStatError::Failed)
    );
    assert_eq!(repo.diff_stat("nope", &base), Err(DiffStatError::Failed));
}

// --- details --------------------------------------------------------------------------------------------------------

fn stat_of(paths: &[(&str, char)]) -> DiffStat {
    DiffStat {
        files_changed: paths.len(),
        insertions: paths.len() as u64,
        deletions: 0,
        truncated: false,
        entries: paths
            .iter()
            .map(|(path, status)| DiffEntry {
                path: path.to_string(),
                status: *status,
                insertions: 1,
                deletions: 0,
            })
            .collect(),
    }
}

#[test]
fn plans_reports_and_packages_are_recorded_with_cleaned_text() {
    let mut data = info(2);
    data["reports"][0]["summary"] = json!("line one\nline\ttwo\u{1b}[31m");
    let packages: HashMap<String, String> = [("r1".to_string(), "pkg-a".to_string())].into();
    let value = details(
        &data,
        &packages,
        Err(&DiffStatError::Failed),
        &ImpactConfig::default(),
    );
    assert_eq!(value["v"], 1);
    assert_eq!(
        value["plans"],
        json!([{"planId": "plan-1", "title": "A title"}])
    );
    assert_eq!(value["plansOmitted"], 0);
    assert_eq!(value["reports"][0]["summary"], "line one line two [31m");
    assert_eq!(value["reports"][0]["packageId"], "pkg-a");
    assert_eq!(value["reports"][1]["packageId"], Value::Null);
    assert_eq!(value["impact"], Value::Null);
    assert_eq!(value["impactError"], "diff_failed");
    let timed = details(
        &data,
        &packages,
        Err(&DiffStatError::Timeout),
        &ImpactConfig::default(),
    );
    assert_eq!(timed["impactError"], "timeout");
    let mut none = info(1);
    none["planId"] = Value::Null;
    let value = details(
        &none,
        &HashMap::new(),
        Err(&DiffStatError::Failed),
        &ImpactConfig::default(),
    );
    assert_eq!(value["plans"], json!([]));
}

#[test]
fn twenty_five_reports_give_twenty_and_five_omitted() {
    let value = details(
        &info(25),
        &HashMap::new(),
        Ok(&stat_of(&[])),
        &ImpactConfig::default(),
    );
    assert_eq!(value["reports"].as_array().unwrap().len(), 20);
    assert_eq!(value["reportsOmitted"], 5);
}

#[test]
fn a_long_path_and_a_multibyte_summary_are_cut_on_a_character() {
    let long = format!("a/{}", "p".repeat(4000));
    let mut data = info(1);
    // 299 ASCII bytes, then a three-byte character that straddles the 300-byte cap.
    data["reports"][0]["summary"] = json!(format!("{}€tail", "s".repeat(299)));
    let stat = stat_of(&[(&long, 'A')]);
    let value = details(&data, &HashMap::new(), Ok(&stat), &ImpactConfig::default());
    let summary = value["reports"][0]["summary"].as_str().unwrap();
    assert_eq!(summary.len(), 299);
    assert_eq!(value["reports"][0]["truncated"], true);
    let area = &value["impact"]["areas"][0];
    assert_eq!(area["path"], "a");
    let config = ImpactConfig {
        areas: vec![long.clone()],
        ..ImpactConfig::default()
    };
    let value = details(&data, &HashMap::new(), Ok(&stat), &config);
    let area = &value["impact"]["areas"][0];
    assert_eq!(area["path"].as_str().unwrap().len(), TEXT_MAX_BYTES);
    assert_eq!(area["truncated"], true);
    let value = details(
        &data,
        &HashMap::new(),
        Ok(&stat),
        &ImpactConfig {
            migrations: vec!["**".into()],
            ..ImpactConfig::default()
        },
    );
    assert_eq!(
        value["impact"]["migrations"][0].as_str().unwrap().len(),
        TEXT_MAX_BYTES
    );
    assert_eq!(value["impact"]["migrationsTruncated"], true);
}

#[test]
fn areas_are_ordered_by_files_then_path_and_capped() {
    let mut paths: Vec<(String, char)> = Vec::new();
    for area in 0..15 {
        for file in 0..=area {
            paths.push((format!("d{area:02}/f{file}"), 'M'));
        }
    }
    let refs: Vec<(&str, char)> = paths.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let value = details(
        &info(1),
        &HashMap::new(),
        Ok(&stat_of(&refs)),
        &ImpactConfig::default(),
    );
    let areas = value["impact"]["areas"].as_array().unwrap();
    assert_eq!(areas.len(), 12);
    assert_eq!(areas[0], json!({"path": "d14", "files": 15}));
    assert_eq!(areas[11]["path"], "d03");
    assert_eq!(value["impact"]["areasOmitted"], 3);
    let tied = stat_of(&[("b/x", 'M'), ("a/x", 'M')]);
    let value = details(
        &info(1),
        &HashMap::new(),
        Ok(&tied),
        &ImpactConfig::default(),
    );
    assert_eq!(value["impact"]["areas"][0]["path"], "a");
}

#[test]
fn migrations_are_added_paths_only_and_capped_at_ten() {
    let paths: Vec<String> = (0..13).map(|n| format!("migrations/{n:04}.sql")).collect();
    let mut entries: Vec<(&str, char)> = paths.iter().map(|p| (p.as_str(), 'A')).collect();
    entries.push(("migrations/9999.sql", 'M'));
    let value = details(&info(1), &HashMap::new(), Ok(&stat_of(&entries)), &config());
    assert_eq!(value["impact"]["migrations"].as_array().unwrap().len(), 10);
    assert_eq!(value["impact"]["migrationsOmitted"], 3);
}

#[test]
fn the_worst_case_stays_within_sixteen_kib() {
    // Everything at its cap, with text that doubles when escaped.
    let quote_path = |n: usize| format!("{}{n:03}", "\"".repeat(296));
    let mut data = info(25);
    for report in data["reports"].as_array_mut().unwrap() {
        report["summary"] = json!("\"".repeat(400));
        report["reportId"] = json!("i".repeat(400));
    }
    data["planTitle"] = json!("\"".repeat(400));
    let areas: Vec<String> = (0..20).map(quote_path).collect();
    let migrations: Vec<String> = (0..20).map(|n| format!("m/{}", quote_path(n))).collect();
    let mut entries: Vec<(&str, char)> = areas.iter().map(|p| (p.as_str(), 'M')).collect();
    entries.extend(migrations.iter().map(|p| (p.as_str(), 'A')));
    let config = ImpactConfig {
        areas: Vec::new(),
        migrations: vec!["m/**".into()],
        next_steps: (0..10)
            .map(|n| NextStep {
                paths: vec!["**".into()],
                step: format!("{n}{}", "\"".repeat(198)),
            })
            .collect(),
    };
    let value = details(&data, &HashMap::new(), Ok(&stat_of(&entries)), &config);
    assert!(size(&value) <= DETAILS_MAX_BYTES, "{} bytes", size(&value));
    assert!(value["reportsOmitted"].as_u64().unwrap() >= 5);
}

// --- the insert in the finishing mutation --------------------------------------------------------------------------

fn seed_running(f: &mut Fixture, ids: &[&str]) {
    f.close();
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let project: String = db
        .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
        .unwrap();
    for (index, id) in ids.iter().enumerate() {
        db.execute(
            "INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, created_at)
             VALUES (?, ?, ?, ?, ?, 'pm-1', 'running', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![
                project,
                id,
                index as i64 + 1,
                "4".repeat(40),
                format!("capstan/integration/{id}")
            ],
        )
        .unwrap();
        // Only one integration may run at a time: settle all but the last into failed.
        if index + 1 < ids.len() {
            db.execute(
                "UPDATE integrations SET state = 'failed', failure_reason = 'x', completed_at = 't' WHERE integration_id = ?",
                [id],
            )
            .unwrap();
        }
    }
    drop(db);
    f.reopen();
}

fn stored(f: &Fixture, id: &str) -> Vec<(String, String)> {
    let db = rusqlite::Connection::open(f.database()).unwrap();
    let mut statement = db
        .prepare(
            "SELECT details_json, computed_at FROM integration_details WHERE integration_id = ?",
        )
        .unwrap();
    let rows = statement
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

fn merged(sha: &str) -> Value {
    json!({"kind": "merged", "headSha": sha})
}

fn finish(f: &Fixture, id: &str, outcome: Value, details: Option<&Value>) -> Result<Value, String> {
    let core = f.core();
    finish_integration_with_details(
        core.kernel(),
        &context(core, OWNER),
        &json!({"integrationId": id, "outcome": outcome}),
        details,
    )
    .map_err(|e| e.message().to_string())
}

#[test]
fn a_merged_finish_stores_the_details_once_and_reads_them_back() {
    let mut f = Fixture::new();
    seed_running(&mut f, &["int-a"]);
    let data = details(
        &info(2),
        &HashMap::new(),
        Ok(&stat_of(&[("a/b", 'M')])),
        &ImpactConfig::default(),
    );
    let record = finish(&f, "int-a", merged(&"5".repeat(40)), Some(&data)).unwrap();
    assert_eq!(record["state"], "merged");
    let rows = stored(&f, "int-a");
    assert_eq!(rows.len(), 1);
    let read: Value = serde_json::from_str(&rows[0].0).unwrap();
    assert_eq!(read, data);
    assert_eq!(f.core().integration_details("int-a").unwrap(), Some(data));
    assert_eq!(f.core().integration_details("missing").unwrap(), None);
}

#[test]
fn the_request_the_result_and_the_event_do_not_depend_on_the_details() {
    let mut with = Fixture::new();
    let mut without = Fixture::new();
    seed_running(&mut with, &["int-a"]);
    seed_running(&mut without, &["int-a"]);
    let data = details(
        &info(1),
        &HashMap::new(),
        Ok(&stat_of(&[("a", 'M')])),
        &ImpactConfig::default(),
    );
    let sha = "5".repeat(40);
    let one = finish(&with, "int-a", merged(&sha), Some(&data)).unwrap();
    let two = finish(&without, "int-a", merged(&sha), None).unwrap();
    assert_eq!(one["state"], two["state"]);
    assert_eq!(one["headSha"], two["headSha"]);
    assert!(stored(&without, "int-a").is_empty());
    let dump = |f: &Fixture| -> (Vec<String>, Vec<String>) {
        let db = rusqlite::Connection::open(f.database()).unwrap();
        let requests = db
            .prepare("SELECT request_hash FROM mutation_requests WHERE idempotency_key LIKE 'idem-%' ORDER BY idempotency_key")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let events = db
            .prepare("SELECT entity_id || '|' || COALESCE(from_state, '') || '|' || COALESCE(to_state, '') || '|' || COALESCE(payload_json, '') FROM controller_events WHERE entity_id = 'int-a' ORDER BY sequence")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        (requests, events)
    };
    let (with_requests, with_events) = dump(&with);
    let (without_requests, without_events) = dump(&without);
    // The hashes carry the fixture's own project, so only the count of recorded requests is comparable; the retry test pins the request itself.
    assert_eq!(with_requests.len(), without_requests.len());
    assert!(!with_requests.is_empty());
    assert!(with_events.iter().any(|e| e.contains("headSha")));
    assert_eq!(with_events, without_events);
}

#[test]
fn a_retry_of_the_same_finish_is_answered_from_the_stored_request() {
    let mut f = Fixture::new();
    seed_running(&mut f, &["int-a"]);
    let data = details(
        &info(1),
        &HashMap::new(),
        Ok(&stat_of(&[("a", 'M')])),
        &ImpactConfig::default(),
    );
    let core = f.core();
    let ctx = context(core, OWNER);
    let input = json!({"integrationId": "int-a", "outcome": merged(&"5".repeat(40))});
    let first = finish_integration_with_details(core.kernel(), &ctx, &input, Some(&data)).unwrap();
    let again = finish_integration_with_details(core.kernel(), &ctx, &input, Some(&data)).unwrap();
    assert_eq!(first, again);
    assert_eq!(stored(&f, "int-a").len(), 1);
}

#[test]
fn only_a_merge_stores_details() {
    let mut f = Fixture::new();
    seed_running(&mut f, &["int-failed", "int-running"]);
    let data = details(
        &info(1),
        &HashMap::new(),
        Err(&DiffStatError::Failed),
        &ImpactConfig::default(),
    );
    // A finish that is not a merge stores nothing, whatever details it is handed.
    let failed =
        json!({"kind": "failed", "reason": "the daemon stopped while the integration was merging"});
    f.close();
    f.reopen();
    finish(&f, "int-running", failed, Some(&data)).unwrap();
    assert!(stored(&f, "int-running").is_empty());
    assert!(stored(&f, "int-failed").is_empty());
}

#[test]
fn a_recovered_finish_leaves_no_row_and_a_later_retry_writes_none() {
    let mut f = Fixture::new();
    seed_running(&mut f, &["int-a"]);
    let failed =
        json!({"kind": "failed", "reason": "the daemon stopped while the integration was merging"});
    finish(&f, "int-a", failed.clone(), None).unwrap();
    assert!(stored(&f, "int-a").is_empty());
    // The integration is no longer running: a late merged finish with details is refused and stores nothing.
    let data = details(
        &info(1),
        &HashMap::new(),
        Err(&DiffStatError::Failed),
        &ImpactConfig::default(),
    );
    assert!(finish(&f, "int-a", merged(&"5".repeat(40)), Some(&data)).is_err());
    assert!(stored(&f, "int-a").is_empty());
}

#[test]
fn details_for_a_row_already_present_are_ignored_without_tripping_the_trigger() {
    let mut f = Fixture::new();
    seed_running(&mut f, &["int-a"]);
    let first = details(
        &info(1),
        &HashMap::new(),
        Err(&DiffStatError::Failed),
        &ImpactConfig::default(),
    );
    f.close();
    {
        let db = rusqlite::Connection::open(f.database()).unwrap();
        let project: String = db
            .query_row("SELECT project_id FROM projects", [], |r| r.get(0))
            .unwrap();
        // A row that is already there while the integration still runs (the window a crash could leave).
        db.execute(
            "INSERT INTO integration_details(project_id, integration_id, details_json, computed_at) VALUES (?, 'int-a', ?, 't')",
            rusqlite::params![project, serde_json::to_string(&first).unwrap()],
        )
        .unwrap();
    }
    f.reopen();
    let second = details(
        &info(3),
        &HashMap::new(),
        Err(&DiffStatError::Timeout),
        &ImpactConfig::default(),
    );
    finish(&f, "int-a", merged(&"5".repeat(40)), Some(&second)).unwrap();
    let rows = stored(&f, "int-a");
    assert_eq!(rows.len(), 1);
    let read: Value = serde_json::from_str(&rows[0].0).unwrap();
    assert_eq!(read, first);
}

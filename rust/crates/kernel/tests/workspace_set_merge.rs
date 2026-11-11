//! Integration sets in the kernel: the landed test, the per-repo integration and the set loop over two scratch
//! repositories. The ledger and git are real; ids come from a counter so the branch names are known.

#[path = "rust_only/fixture.rs"]
mod fixture;

use capstan_config::ImpactConfig;
use capstan_kernel::env::Env;
use capstan_kernel::integrate::{
    integrate_repo, integrate_set, integration_branch_name, member_branch, with_suffix, GitRepo,
    GitSeams, IntegrateError, IntegrationDeps, IntegrationState, Landed, MemberStop, MemberTarget,
    SetRun,
};
use capstan_kernel::{Core, KernelOptions, SystemEnv};
use fixture::{context, step, OWNER};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

struct CountingEnv(Arc<AtomicU64>);

impl Env for CountingEnv {
    fn now(&self) -> i64 {
        SystemEnv.now()
    }
    fn uuid(&self) -> String {
        format!(
            "00000000-0000-4000-8000-{:012}",
            self.0.fetch_add(1, Ordering::SeqCst)
        )
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        SystemEnv.random_bytes(n)
    }
}

fn git_env() -> Vec<(&'static str, String)> {
    vec![
        ("PATH", std::env::var("PATH").unwrap_or_default()),
        ("LC_ALL", "C".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("GIT_TERMINAL_PROMPT", "0".into()),
        ("GIT_AUTHOR_NAME", "Test".into()),
        ("GIT_AUTHOR_EMAIL", "test@example.com".into()),
        ("GIT_COMMITTER_NAME", "Test".into()),
        ("GIT_COMMITTER_EMAIL", "test@example.com".into()),
    ]
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(git_env())
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "-b", "main"]);
    std::fs::write(dir.join("f.txt"), "1\n2\n3\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "chore: initial commit"]);
}

/// A commit on its own branch off `from` that writes `file`; main is checked out again afterwards.
fn commit_on(dir: &Path, from: &str, file: &str, text: &str) -> String {
    let branch = format!("w-{}", git(dir, &["rev-parse", "--short=12", "HEAD"]));
    let _ = Command::new("git")
        .args(["branch", "-D", &branch])
        .current_dir(dir)
        .env_clear()
        .envs(git_env())
        .output();
    git(dir, &["checkout", "--quiet", "-b", &branch, from]);
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", "-A"]);
    git(
        dir,
        &["commit", "--quiet", "-m", &format!("feat: write {file}")],
    );
    let sha = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["checkout", "--quiet", "main"]);
    sha
}

fn tip(dir: &Path, branch: &str) -> String {
    git(dir, &["rev-parse", branch])
}

fn has_branch(dir: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(dir)
        .env_clear()
        .envs(git_env())
        .output()
        .unwrap()
        .status
        .success()
}

struct World {
    _dir: tempfile::TempDir,
    core: Core,
    uuids: Arc<AtomicU64>,
    api: PathBuf,
    web: PathBuf,
    state: IntegrationState,
    next_sequence: std::cell::Cell<i64>,
}

impl World {
    fn new() -> World {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700)).unwrap();
        let uuids = Arc::new(AtomicU64::new(1));
        let core = Core::open(
            &state,
            &fixture::initial(),
            &KernelOptions::default(),
            Box::new(CountingEnv(uuids.clone())),
        )
        .expect("the ledger opens");
        step(
            &core,
            "syncRoleDefinitions",
            json!([[
                {"name": "pm", "kind": "PM", "host": "claude", "configHash": "b".repeat(64)},
                {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
            ]]),
        );
        for (agent, role, kind) in [
            ("pm-1", "pm", "PM"),
            ("dev-1", "developer", "Developer"),
            ("dev-2", "developer", "Developer"),
        ] {
            let seat = format!("seat-{agent}");
            step(
                &core,
                "createSeat",
                json!([{"seatId": seat, "name": seat, "role": kind}]),
            );
            let actor = step(
                &core,
                "createActor",
                json!([{"displayName": agent, "role": kind, "seatId": seat}]),
            );
            step(
                &core,
                "registerAgent",
                json!([{"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}]),
            );
        }
        let (api, web) = (dir.path().join("api"), dir.path().join("web"));
        make_repo(&api);
        make_repo(&web);
        World {
            _dir: dir,
            core,
            uuids,
            api,
            web,
            state: IntegrationState::new(),
            next_sequence: std::cell::Cell::new(1),
        }
    }

    fn db(&self) -> rusqlite::Connection {
        let path = capstan_ledger::resolve_database_path(&self._dir.path().join("state")).unwrap();
        rusqlite::Connection::open(path).unwrap()
    }

    fn root(&self, repo: &str) -> &Path {
        if repo == "api" {
            &self.api
        } else {
            &self.web
        }
    }

    /// An accepted, reviewed report of `agent` naming one commit per repo.
    fn report(&self, agent: &str, commits: &[(&str, &str)]) -> String {
        self.report_rows(agent, commits[0].1, commits)
    }

    /// A report of the time before workspace mode: no `report_commits` rows.
    fn report_without_commits(&self, agent: &str, sha: &str) -> String {
        self.report_rows(agent, sha, &[])
    }

    fn report_rows(&self, agent: &str, sha: &str, commits: &[(&str, &str)]) -> String {
        let conn = self.db();
        let actor: String = conn
            .query_row(
                "SELECT actor_id FROM agents WHERE agent_id = ?",
                [agent],
                |r| r.get(0),
            )
            .unwrap();
        let sequence = self.next_sequence.get();
        self.next_sequence.set(sequence + 1);
        let id = format!("report-{sequence}");
        conn.execute(
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES ('prustonly', ?, ?, ?, 1, ?, ?, 'b', ?, 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![id, sequence, agent, actor, sha, format!("summary {id}")],
        )
        .unwrap();
        for (repo, sha) in commits {
            conn.execute(
                "INSERT INTO report_commits(project_id, report_id, repo, commit_sha, branch, base_sha) VALUES ('prustonly', ?, ?, ?, 'b', ?)",
                rusqlite::params![id, repo, sha, "0".repeat(40)],
            )
            .unwrap();
        }
        let reviewer: String = conn
            .query_row(
                "SELECT actor_id FROM agents WHERE agent_id = 'pm-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, author_agent_id, author_actor_id, verdict_text, state, created_at)
             VALUES ('prustonly', ?, ?, 1, ?, ?, ?, ?, 'pm', 'pm-1', ?, ?, ?, 'ok', 'passed', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![format!("rv-{sequence}"), sequence, id, sha, "0".repeat(40), actor, reviewer, agent, actor],
        )
        .unwrap();
        id
    }

    fn set_run<'a>(
        &'a self,
        ids: &'a [String],
        order: &'a [String],
        main: &'a dyn Fn(&str) -> String,
        git_for: &'a dyn Fn(&str) -> GitRepo,
        before: &'a dyn Fn(&str) -> Result<(), MemberStop>,
    ) -> SetRun<'a> {
        SetRun {
            report_ids: ids,
            requested_by: "pm-1",
            merge_order: order,
            main_branch_of: main,
            git_for,
            before_member: before,
        }
    }

    /// Runs `integrate_set` with plain git, `main` as every main branch and the given order.
    fn integrate_with(
        &self,
        ids: &[String],
        order: &[&str],
        before: &dyn Fn(&str) -> Result<(), MemberStop>,
        seams: Option<GitSeams>,
    ) -> Result<Value, IntegrateError> {
        let order: Vec<String> = order.iter().map(|s| s.to_string()).collect();
        let main = |_: &str| "main".to_string();
        let git_for = |repo: &str| {
            let git = GitRepo::new(self.root(repo));
            match &seams {
                Some(seams) => git.with_seams(seams.clone()),
                None => git,
            }
        };
        let root = GitRepo::new(&self.api);
        let ctx = |credential: &str| context(&self.core, credential);
        let log = |_: &str, _: Value| {};
        let impact = ImpactConfig::default();
        let deps = IntegrationDeps {
            kernel: self.core.kernel(),
            git: &root,
            state: &self.state,
            credential: OWNER,
            context: &ctx,
            log: &log,
            impact: &impact,
        };
        integrate_set(&deps, &self.set_run(ids, &order, &main, &git_for, before))
    }

    fn integrate(&self, ids: &[String], order: &[&str]) -> Value {
        self.integrate_with(ids, order, &|_| Ok(()), None)
            .expect("the set is integrated")
    }

    fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }
}

// ------------------------------------------------------------------------------------------------ landed

fn landed(repo: &Path, head: &str) -> Landed {
    GitRepo::new(repo)
        .tree_landed(&tip(repo, "main"), head)
        .expect("git answers")
}

#[test]
fn landed_means_main_already_holds_the_change() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("r");
    make_repo(&repo);
    let base = tip(&repo, "main");

    // Not landed: main does not hold the change.
    let head = commit_on(&repo, "main", "f.txt", "one\n2\n3\n");
    assert_eq!(landed(&repo, &head), Landed::No);

    // Squash-merged: main holds a squash of head.
    git(&repo, &["merge", "--squash", "--quiet", &head]);
    git(&repo, &["commit", "--quiet", "-m", "feat: squash"]);
    assert_eq!(landed(&repo, &head), Landed::Yes);
    // Squash-merged, then main edited the same line: not landed (conflict).
    std::fs::write(repo.join("f.txt"), "uno\n2\n3\n").unwrap();
    git(&repo, &["commit", "--quiet", "-am", "feat: edit again"]);
    assert_eq!(landed(&repo, &head), Landed::No);

    // Fast-forward: main is at head.
    git(&repo, &["reset", "--quiet", "--hard", &base]);
    git(&repo, &["merge", "--quiet", "--ff-only", &head]);
    assert_eq!(landed(&repo, &head), Landed::Yes);

    // Rebased: main holds head's change on top of another commit.
    git(&repo, &["reset", "--quiet", "--hard", &base]);
    std::fs::write(repo.join("g.txt"), "g\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "--quiet", "-m", "feat: other"]);
    git(&repo, &["cherry-pick", &head]);
    assert_ne!(tip(&repo, "main"), head);
    assert_eq!(landed(&repo, &head), Landed::Yes);
}

#[test]
fn a_git_whose_merge_tree_fails_is_unknown() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("r");
    make_repo(&repo);
    let head = commit_on(&repo, "main", "a.txt", "a\n");
    let fake = dir.path().join("fake-git");
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::fs::write(
        &fake,
        format!("#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = merge-tree ] && exit 2; done\nexec {} \"$@\"\n", real.trim()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let git = GitRepo::new(&repo).with_seams(GitSeams {
        git_program: Some(fake),
        skip_version_probe: true,
        ..GitSeams::default()
    });
    assert_eq!(
        git.tree_landed(&tip(&repo, "main"), &head).unwrap(),
        Landed::Unknown(2)
    );
}

// ------------------------------------------------------------------------------------------------ the loop

#[test]
fn members_merge_in_order_each_with_its_own_branch_and_main_stays() {
    let w = World::new();
    let (a, b) = (
        commit_on(&w.api, "main", "a.txt", "a\n"),
        commit_on(&w.web, "main", "b.txt", "b\n"),
    );
    let report = w.report("dev-1", &[("api", &a), ("web", &b)]);
    let (api_main, web_main) = (tip(&w.api, "main"), tip(&w.web, "main"));
    let first_id = w.uuids.load(Ordering::SeqCst);
    let answer = w.integrate(std::slice::from_ref(&report), &["web", "api"]);
    assert_eq!(answer["state"], "merged", "{answer}");
    let base = integration_branch_name(&format!("00000000-0000-4000-8000-{first_id:012}"), None);
    assert_eq!(answer["branch"], base);
    let members = answer["members"].as_array().unwrap();
    let repos: Vec<&str> = members
        .iter()
        .map(|m| m["repo"].as_str().unwrap())
        .collect();
    assert_eq!(repos, ["web", "api"], "the merge order decides");
    assert_eq!(members[0]["branch"], member_branch(&base, "web", 1));
    assert_eq!(members[1]["branch"], format!("{base}-api"));
    assert!(has_branch(&w.web, &format!("{base}-web")));
    assert!(has_branch(&w.api, &format!("{base}-api")));
    assert!(
        !has_branch(&w.api, &format!("{base}-web")),
        "each branch lives in its own repo"
    );
    // Main is never touched.
    assert_eq!(
        (tip(&w.api, "main"), tip(&w.web, "main")),
        (api_main, web_main)
    );
    // One ordinary integrations row per member, merged, in the set.
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'merged'"),
        2
    );
    let set = w
        .core
        .integration_set(answer["setId"].as_str().unwrap())
        .unwrap();
    assert_eq!(set["state"], "merged");
    assert_eq!(set["members"][0]["attempt"], 1);
    assert!(!w.state.busy());
}

#[test]
fn a_repo_the_reports_do_not_touch_has_no_member() {
    let w = World::new();
    let a = commit_on(&w.api, "main", "a.txt", "a\n");
    let report = w.report("dev-1", &[("api", &a)]);
    let answer = w.integrate(&[report], &["web", "api"]);
    assert_eq!(answer["members"].as_array().unwrap().len(), 1);
    assert_eq!(answer["members"][0]["repo"], "api");
}

#[test]
fn member_branches_of_two_repos_never_collide_under_a_long_title() {
    let base = format!("integration/{}", "a-very-long-set-name-".repeat(6));
    assert!(base.len() >= 100, "{base}");
    let (api, web) = (
        member_branch(&base, "api", 1),
        member_branch(&base, "web", 1),
    );
    assert_ne!(api, web);
    assert!(
        api.ends_with("-api") && web.ends_with("-web"),
        "{api} {web}"
    );
    assert!(api.len() <= 100 && web.len() <= 100);
    let again = member_branch(&base, "api", 2);
    assert!(again.ends_with("-api-2") && again.len() <= 100, "{again}");
}

#[test]
fn a_set_with_every_branch_suffix_taken_is_refused() {
    let w = World::new();
    let a = commit_on(&w.api, "main", "a.txt", "a\n");
    let report = w.report("dev-1", &[("api", &a)]);
    let first_id = w.uuids.load(Ordering::SeqCst);
    let base = integration_branch_name(&format!("00000000-0000-4000-8000-{first_id:012}"), None);
    for n in 1..1000 {
        // Creating a set uses ids; the run must see the same next id again.
        w.core
            .create_integration_set(
                &context(&w.core, OWNER),
                &json!({"branch": with_suffix(&base, n), "repos": ["api"]}),
            )
            .unwrap();
    }
    w.uuids.store(first_id, Ordering::SeqCst);
    let error = w
        .integrate_with(&[report], &["api"], &|_| Ok(()), None)
        .unwrap_err();
    assert!(
        error.display().contains("no free branch name for set"),
        "{}",
        error.display()
    );
    assert!(!has_branch(&w.api, &base));
}

#[test]
fn a_landed_repo_is_skipped_and_all_landed_is_refused() {
    let w = World::new();
    let (a, b) = (
        commit_on(&w.api, "main", "a.txt", "a\n"),
        commit_on(&w.web, "main", "b.txt", "b\n"),
    );
    let report = w.report("dev-1", &[("api", &a), ("web", &b)]);
    git(&w.web, &["merge", "--quiet", "--ff-only", &b]);
    let answer = w.integrate(std::slice::from_ref(&report), &["api", "web"]);
    assert_eq!(answer["landed"], json!(["web"]));
    assert_eq!(answer["members"].as_array().unwrap().len(), 1);
    assert_eq!(answer["members"][0]["repo"], "api");
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 1);

    let w = World::new();
    let a = commit_on(&w.api, "main", "a.txt", "a\n");
    let report = w.report("dev-1", &[("api", &a)]);
    git(&w.api, &["merge", "--quiet", "--ff-only", &a]);
    let error = w
        .integrate_with(&[report], &["api"], &|_| Ok(()), None)
        .unwrap_err();
    assert!(
        error
            .display()
            .contains("nothing to integrate: every repo already has these commits"),
        "{}",
        error.display()
    );
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
}

#[test]
fn a_branch_that_exists_refuses_before_any_set_is_made() {
    let w = World::new();
    let (a, b) = (
        commit_on(&w.api, "main", "a.txt", "a\n"),
        commit_on(&w.web, "main", "b.txt", "b\n"),
    );
    let report = w.report("dev-1", &[("api", &a), ("web", &b)]);
    // The set branch is built from the next id the ledger hands out.
    let next = w.uuids.load(Ordering::SeqCst);
    let base = integration_branch_name(&format!("00000000-0000-4000-8000-{next:012}"), None);
    git(&w.web, &["branch", &format!("{base}-web"), "main"]);
    let error = w
        .integrate_with(&[report], &["api", "web"], &|_| Ok(()), None)
        .unwrap_err();
    assert!(
        error
            .display()
            .contains(&format!("branch {base}-web already exists in repo web")),
        "{}",
        error.display()
    );
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
    assert_eq!(w.count("SELECT count(*) FROM integrations"), 0);
    assert!(
        !has_branch(&w.api, &format!("{base}-api")),
        "nothing was made in the first repo"
    );
}

#[test]
fn a_missing_main_branch_and_an_old_git_refuse() {
    let w = World::new();
    let a = commit_on(&w.api, "main", "a.txt", "a\n");
    let report = w.report("dev-1", &[("api", &a)]);
    git(&w.api, &["branch", "-m", "main", "trunk"]);
    let error = w
        .integrate_with(std::slice::from_ref(&report), &["api"], &|_| Ok(()), None)
        .unwrap_err();
    assert!(
        error.display().contains("repo api has no branch main"),
        "{}",
        error.display()
    );
    git(&w.api, &["branch", "-m", "trunk", "main"]);
    let error = w
        .integrate_with(
            &[report],
            &["api"],
            &|_| Ok(()),
            Some(GitSeams {
                force_old_git: true,
                ..GitSeams::default()
            }),
        )
        .unwrap_err();
    assert!(
        error
            .display()
            .contains("git 2.38 or newer is needed for workspace integrate"),
        "{}",
        error.display()
    );
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
}

#[test]
fn a_report_without_commit_rows_predates_workspace_mode() {
    let w = World::new();
    let a = commit_on(&w.api, "main", "a.txt", "a\n");
    let report = w.report_without_commits("dev-1", &a);
    let error = w
        .integrate_with(std::slice::from_ref(&report), &["api"], &|_| Ok(()), None)
        .unwrap_err();
    assert!(
        error.display().contains(&format!(
            "report {report} predates workspace mode; report again"
        )),
        "{}",
        error.display()
    );
}

#[test]
fn a_conflict_in_the_second_repo_keeps_the_first_merged() {
    let w = World::new();
    let a = commit_on(&w.api, "main", "a.txt", "a\n");
    let b = commit_on(&w.web, "main", "f.txt", "two\n2\n3\n");
    let report = w.report("dev-1", &[("api", &a), ("web", &b)]);
    // Main of web moved on in the same line after the report's base.
    std::fs::write(w.web.join("f.txt"), "uno\n2\n3\n").unwrap();
    git(&w.web, &["commit", "--quiet", "-am", "feat: edit"]);
    let answer = w.integrate(&[report], &["api", "web"]);
    assert_eq!(answer["state"], "conflicted", "{answer}");
    assert_eq!(answer["members"][0]["state"], "merged");
    assert_eq!(answer["members"][1]["state"], "conflicted");
    assert_eq!(answer["members"][1]["conflict"]["files"], json!(["f.txt"]));
    let set_id = answer["setId"].as_str().unwrap();
    assert_eq!(
        answer["next"],
        format!(
            "resolve with a new report for repo web, then cstan integrate redo {set_id} --repo web"
        )
    );
    assert!(answer["reason"].as_str().unwrap().contains("f.txt"));
    assert_eq!(answer["merged"], json!(["api"]));
    assert_eq!(answer["failed"], json!(["web"]));
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'merged'"),
        1
    );
}

#[test]
#[allow(clippy::type_complexity)]
fn an_error_a_cancel_and_a_panic_settle_the_running_row_and_fail_the_set() {
    let cases: [(&str, fn(&str) -> Result<(), MemberStop>, &str); 3] = [
        (
            "error",
            |_| Err(MemberStop::Error("disk full".into())),
            "disk full",
        ),
        ("cancel", |_| Err(MemberStop::Cancel), "cancelled"),
        ("panic", |_| panic!("boom"), "boom"),
    ];
    for (name, before, reason) in cases {
        let w = World::new();
        let (a, b) = (
            commit_on(&w.api, "main", "a.txt", "a\n"),
            commit_on(&w.web, "main", "b.txt", "b\n"),
        );
        let report = w.report("dev-1", &[("api", &a), ("web", &b)]);
        // The first member merges; the fault hits the second.
        let calls = std::cell::Cell::new(0);
        let hook = |repo: &str| {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                before(repo)
            } else {
                Ok(())
            }
        };
        let hook: &dyn Fn(&str) -> Result<(), MemberStop> = &hook;
        let answer = w
            .integrate_with(&[report], &["api", "web"], hook, None)
            .unwrap_or_else(|e| panic!("{name}: {}", e.display()));
        assert_eq!(answer["state"], "failed", "{name}: {answer}");
        assert!(
            answer["reason"].as_str().unwrap().contains(reason),
            "{name}: {answer}"
        );
        assert_eq!(answer["members"][0]["state"], "merged", "{name}");
        assert_eq!(
            w.count("SELECT count(*) FROM integrations WHERE state = 'running'"),
            0,
            "{name}"
        );
        assert_eq!(
            w.count("SELECT count(*) FROM integrations WHERE state = 'failed'"),
            1,
            "{name}"
        );
        assert_eq!(
            w.count("SELECT count(*) FROM integration_sets WHERE state = 'failed'"),
            1,
            "{name}"
        );
        assert!(!w.state.busy(), "{name}: the guard is free");
        let set_id = answer["setId"].as_str().unwrap();
        assert!(
            answer["next"]
                .as_str()
                .unwrap()
                .contains(&format!("integrate redo {set_id} --repo web")),
            "{name}"
        );
    }
}

// ------------------------------------------------------------------------------------------ integrate_repo

fn member_run(w: &World, set: &str, ids: &[(String, String)]) -> Result<Value, IntegrateError> {
    let git = GitRepo::new(&w.api);
    let ctx = |credential: &str| context(&w.core, credential);
    let log = |_: &str, _: Value| {};
    let impact = ImpactConfig::default();
    let deps = IntegrationDeps {
        kernel: w.core.kernel(),
        git: &git,
        state: &w.state,
        credential: OWNER,
        context: &ctx,
        log: &log,
        impact: &impact,
    };
    let target = MemberTarget {
        set_id: set,
        branch: "integration/member-api",
        main_branch: "main",
        base_sha: &tip(&w.api, "main"),
    };
    integrate_repo(&deps, "api", ids, "pm-1", &target)
}

fn new_set(w: &World) -> String {
    let set = w
        .core
        .create_integration_set(
            &context(&w.core, OWNER),
            &json!({"branch": "integration/member", "repos": ["api"]}),
        )
        .unwrap();
    set["setId"].as_str().unwrap().to_string()
}

#[test]
fn the_newest_report_of_an_agent_supersedes_and_other_agents_each_count() {
    // One agent, two reports: only the newest is merged.
    let w = World::new();
    let c1 = commit_on(&w.api, "main", "a.txt", "a\n");
    let c2 = commit_on(&w.api, &c1, "a2.txt", "a2\n");
    let (r1, r2) = (
        w.report("dev-1", &[("api", &c1)]),
        w.report("dev-1", &[("api", &c2)]),
    );
    let set = new_set(&w);
    let record = member_run(
        &w,
        &set,
        &[(r1.clone(), c1.clone()), (r2.clone(), c2.clone())],
    )
    .unwrap();
    assert_eq!(record["state"], "merged", "{record}");
    let reports: Vec<&str> = record["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["reportId"].as_str().unwrap())
        .collect();
    assert_eq!(reports, [r2.as_str()]);

    // Two agents: both, in sequence order.
    let w = World::new();
    let c1 = commit_on(&w.api, "main", "a.txt", "a\n");
    let c3 = commit_on(&w.api, "main", "c.txt", "c\n");
    let (r1, r3) = (
        w.report("dev-1", &[("api", &c1)]),
        w.report("dev-2", &[("api", &c3)]),
    );
    let set = new_set(&w);
    let record = member_run(&w, &set, &[(r3.clone(), c3), (r1.clone(), c1)]).unwrap();
    assert_eq!(record["state"], "merged", "{record}");
    let reports: Vec<&str> = record["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["reportId"].as_str().unwrap())
        .collect();
    assert_eq!(
        reports,
        [r1.as_str(), r3.as_str()],
        "sequence order, not the order given"
    );
}

#[test]
fn a_member_cannot_be_settled_on_its_own() {
    let w = World::new();
    let a = commit_on(&w.api, "main", "a.txt", "a\n");
    let report = w.report("dev-1", &[("api", &a)]);
    let answer = w.integrate(&[report], &["api"]);
    let id = answer["members"][0]["integrationId"]
        .as_str()
        .unwrap()
        .to_string();
    let git = GitRepo::new(&w.api);
    let ctx = |credential: &str| context(&w.core, credential);
    let log = |_: &str, _: Value| {};
    let impact = ImpactConfig::default();
    let deps = IntegrationDeps {
        kernel: w.core.kernel(),
        git: &git,
        state: &w.state,
        credential: OWNER,
        context: &ctx,
        log: &log,
        impact: &impact,
    };
    for outcome in ["confirmed", "discarded"] {
        let error = capstan_kernel::integrate::settle_integration(&deps, &id, outcome).unwrap_err();
        let set_id = answer["setId"].as_str().unwrap();
        assert!(
            error.display().contains(&format!(
                "integration {id} belongs to set {set_id}; confirm and discard per repo come with plan-184"
            )),
            "{}",
            error.display()
        );
    }
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'merged'"),
        1
    );
}

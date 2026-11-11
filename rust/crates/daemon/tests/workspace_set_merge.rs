//! `cstan integrate <report-id>...` in a workspace project: a set of the repos the reports touch, merged one member at
//! a time. The kernel, the daemon handler and git are real; the faults of the set loop and the git of its members are
//! the seams `install_integrate_seams` sets.

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::integrate_seams::{
    install_integrate_seams, IntegrateSeams, SetFault,
};
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_kernel::integrate::GitSeams;
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

static WORLDS: AtomicUsize = AtomicUsize::new(0);

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
        "git {args:?} in {}: {}",
        cwd.display(),
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

/// A commit off main on a throwaway branch writing `file`; main is checked out again.
fn commit_on(dir: &Path, file: &str, text: &str) -> String {
    let branch = format!("w-{}", git(dir, &["rev-parse", "--short=12", "HEAD"]));
    git(dir, &["checkout", "--quiet", "-B", &branch, "main"]);
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
    _dir: common::PrivateDir,
    scratch: common::PrivateDir,
    deps: Deps,
    operator: String,
    database: PathBuf,
    repos: PathBuf,
    root: PathBuf,
    sequence: std::cell::Cell<i64>,
}

impl World {
    fn new() -> World {
        World::build(true)
    }

    /// `workspace` false writes a project with no `[repos]`: its one repository is the project root.
    fn build(workspace: bool) -> World {
        let dir = private_tempdir();
        let scratch = private_tempdir();
        let repos = scratch.path().join("repos");
        make_repo(&repos.join("core"));
        make_repo(&repos.join("web"));
        let identity = Identity {
            project_id: "pset".into(),
            name: "Set Merge".into(),
            credential: format!("operator-{}", "s".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let config = format!(
            r#"schema_version = 1

[hosts.claude]
kind = "claude"

[architect]
enabled = false

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

{ws}"#,
            ws = if workspace {
                format!(
                    r#"[repos.core]
path = "{r}/core"
main_branch = "main"

[repos.web]
path = "{r}/web"
main_branch = "main"

[workspace]
default_repo = "core"
merge_order = ["core", "web"]
worktree_dir = "{c}"
"#,
                    r = repos.display(),
                    c = scratch.path().join("changes").display()
                )
            } else {
                String::new()
            },
        );
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let trust: TrustStore = common::kept_trust_store();
        common::trusted_project(dir.path(), &trust);
        build_repo(dir.path(), &json!({"files": {"a.txt": "a\n"}}));
        let database = capstan_ledger::resolve_database_path(&state).unwrap();
        capstan_ledger::open_database(&database, &capstan_ledger::OpenOptions::default()).unwrap();
        let project = identity.initial_project();
        let kernel = {
            let (state, project, root) = (state.clone(), project.clone(), dir.path().to_path_buf());
            KernelHandle::spawn(move || {
                Core::open(
                    &state,
                    &project,
                    &KernelOptions {
                        workspace_root: Some(root),
                        ..KernelOptions::default()
                    },
                    seeded_env(&format!(
                        "set-merge-{}-{}",
                        std::process::id(),
                        WORLDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    )),
                )
            })
            .expect("the kernel opens")
        };
        let sink: Logger = Arc::new(|_entry: &LogEntry| {});
        let mut options = DaemonOptions::new(state.clone(), project, dir.path().to_path_buf());
        options.trust_store = trust.clone();
        let role_config = capstan_config::load_role_config_with(dir.path(), &trust).unwrap();
        assert_eq!(role_config.workspace.is_some(), workspace);
        options.capstan = Some(Arc::new(role_config));
        let mut deps = Deps::new(kernel, options, sink);
        deps.git = Arc::new(ProjectGit::new(dir.path()));
        let world = World {
            root: dir.path().to_path_buf(),
            _dir: dir,
            scratch,
            deps,
            operator: identity.credential.clone(),
            database,
            repos,
            sequence: std::cell::Cell::new(1),
        };
        world.step(
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
            world.step(
                "createSeat",
                json!([{"seatId": seat, "name": seat, "role": kind}]),
            );
            let actor = world.step(
                "createActor",
                json!([{"displayName": agent, "role": kind, "seatId": seat}]),
            );
            world.step(
                "registerAgent",
                json!([{"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]}]),
            );
        }
        world
    }

    fn step(&self, op: &str, args: Value) -> Value {
        let (credential, name) = (self.operator.clone(), op.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args.as_array().cloned().unwrap());
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn repo(&self, name: &str) -> PathBuf {
        self.repos.join(name)
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.database).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn call(&self, command: &str, args: &[&str]) -> Value {
        let frame = json!({"v": 1, "credential": self.operator, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    /// An accepted, reviewed report of `agent` with one commit per repo (none: a report of before workspace mode).
    fn report(&self, agent: &str, commits: &[(&str, &str)], legacy_sha: Option<&str>) -> String {
        let conn = self.db();
        let actor = |who: &str| -> String {
            conn.query_row(
                "SELECT actor_id FROM agents WHERE agent_id = ?",
                [who],
                |r| r.get(0),
            )
            .unwrap()
        };
        let sequence = self.sequence.get();
        self.sequence.set(sequence + 1);
        let id = format!("report-{sequence}");
        let sha = legacy_sha.unwrap_or_else(|| commits[0].1);
        conn.execute(
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES ('pset', ?, ?, ?, 1, ?, ?, 'b', ?, 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![id, sequence, agent, actor(agent), sha, format!("summary {id}")],
        )
        .unwrap();
        for (repo, sha) in commits {
            conn.execute(
                "INSERT INTO report_commits(project_id, report_id, repo, commit_sha, branch, base_sha) VALUES ('pset', ?, ?, ?, 'b', ?)",
                rusqlite::params![id, repo, sha, "0".repeat(40)],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, author_agent_id, author_actor_id, verdict_text, state, created_at)
             VALUES ('pset', ?, ?, 1, ?, ?, ?, ?, 'pm', 'pm-1', ?, ?, ?, 'ok', 'passed', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![
                format!("rv-{sequence}"), sequence, id, sha, "0".repeat(40), actor(agent), actor("pm-1"), agent, actor(agent)
            ],
        )
        .unwrap();
        id
    }

    /// The reports of a typical change: one commit in each of core and web.
    fn two_repo_report(&self) -> (String, String, String) {
        let (a, b) = (
            commit_on(&self.repo("core"), "a.txt", "a\n"),
            commit_on(&self.repo("web"), "b.txt", "b\n"),
        );
        let report = self.report("dev-1", &[("core", &a), ("web", &b)], None);
        (report, a, b)
    }

    fn mains(&self) -> (String, String) {
        (
            git(&self.repo("core"), &["rev-parse", "main"]),
            git(&self.repo("web"), &["rev-parse", "main"]),
        )
    }

    fn install(&self, seams: IntegrateSeams) {
        install_integrate_seams(&self.deps, seams);
    }
}

fn result(answer: &Value) -> &Value {
    assert_eq!(answer["ok"], true, "{answer}");
    &answer["result"]
}

fn refusal(answer: &Value) -> String {
    assert_eq!(answer["ok"], false, "{answer}");
    answer["message"].as_str().unwrap_or_default().to_string()
}

fn repos_of(set: &Value) -> Vec<&str> {
    set["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["repo"].as_str().unwrap())
        .collect()
}

// ------------------------------------------------------------------------------------------------ merging

#[test]
fn both_repos_merge_one_after_the_other_and_no_main_moves() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    let before = w.mains();
    let answer = w.call("integrate", &[&report]);
    let set = result(&answer);
    assert_eq!(set["state"], "merged", "{answer}");
    assert_eq!(repos_of(set), ["core", "web"], "merge order");
    assert_eq!(set["merged"], json!(["core", "web"]));
    assert_eq!(set["failed"], json!([]));
    let branch = set["branch"].as_str().unwrap();
    assert!(has_branch(&w.repo("core"), &format!("{branch}-core")));
    assert!(has_branch(&w.repo("web"), &format!("{branch}-web")));
    assert_eq!(w.mains(), before, "main is never touched");
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'merged'"),
        2
    );
    assert_eq!(
        w.count("SELECT count(*) FROM integration_set_members WHERE integration_id IS NOT NULL"),
        2
    );
}

#[test]
fn reports_touching_one_repo_make_a_one_member_set() {
    let w = World::new();
    let a = commit_on(&w.repo("web"), "b.txt", "b\n");
    let report = w.report("dev-1", &[("web", &a)], None);
    let answer = w.call("integrate", &[&report]);
    let set = result(&answer);
    assert_eq!(repos_of(set), ["web"]);
    assert_eq!(set["state"], "merged");
    assert!(!has_branch(
        &w.repo("core"),
        &format!("{}-core", set["branch"].as_str().unwrap())
    ));
}

#[test]
fn a_member_is_not_confirmed_discarded_or_merged_on_its_own() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    let answer = w.call("integrate", &[&report]);
    let set = result(&answer);
    let set_id = set["setId"].as_str().unwrap();
    let member = set["members"][0]["integrationId"].as_str().unwrap();
    for verb in ["confirm", "discard"] {
        let refused = w.call("integrate", &[verb, member]);
        let text = refusal(&refused);
        assert!(
            text.contains(&format!(
                "integration {member} belongs to set {set_id}; confirm and discard per repo come with plan-184"
            )),
            "{verb}: {text}"
        );
    }
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'merged'"),
        2
    );
}

#[test]
fn a_repo_without_its_main_branch_refuses_and_makes_nothing() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    git(&w.repo("web"), &["branch", "-m", "main", "trunk"]);
    let text = refusal(&w.call("integrate", &[&report]));
    assert!(text.contains("repo web has no branch main"), "{text}");
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
    assert_eq!(w.count("SELECT count(*) FROM integrations"), 0);
}

#[test]
fn a_conflict_lists_that_repos_files_and_the_next_step_and_keeps_the_first_merged() {
    let w = World::new();
    let a = commit_on(&w.repo("core"), "a.txt", "a\n");
    let b = commit_on(&w.repo("web"), "f.txt", "two\n2\n3\n");
    let report = w.report("dev-1", &[("core", &a), ("web", &b)], None);
    std::fs::write(w.repo("web").join("f.txt"), "uno\n2\n3\n").unwrap();
    git(&w.repo("web"), &["commit", "--quiet", "-am", "feat: edit"]);
    let answer = w.call("integrate", &[&report]);
    let set = result(&answer);
    assert_eq!(set["state"], "conflicted", "{answer}");
    assert_eq!(set["members"][0]["state"], "merged");
    assert_eq!(set["members"][1]["repo"], "web");
    assert_eq!(set["members"][1]["conflict"]["files"], json!(["f.txt"]));
    let set_id = set["setId"].as_str().unwrap();
    assert_eq!(
        set["next"],
        format!(
            "resolve with a new report for repo web, then cstan integrate redo {set_id} --repo web"
        )
    );
    assert_eq!(
        w.count("SELECT count(*) FROM integrations WHERE state = 'merged'"),
        1
    );
}

#[test]
fn a_report_of_before_workspace_mode_is_refused() {
    let w = World::new();
    let a = commit_on(&w.repo("core"), "a.txt", "a\n");
    let report = w.report("dev-1", &[], Some(&a));
    let text = refusal(&w.call("integrate", &[&report]));
    assert!(
        text.contains(&format!(
            "report {report} predates workspace mode; report again"
        )),
        "{text}"
    );
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
}

// ------------------------------------------------------------------------------------------------ faults

#[test]
#[allow(clippy::type_complexity)]
fn an_error_a_panic_and_a_cancel_fail_the_set_settle_the_row_and_free_the_guard() {
    let faults: [(&str, fn() -> SetFault, &str); 3] = [
        ("error", || SetFault::Error("disk full".into()), "disk full"),
        ("panic", || SetFault::Panic, "injected panic"),
        ("cancel", || SetFault::Cancel, "cancelled"),
    ];
    for (name, fault, reason) in faults {
        let w = World::new();
        let (report, _, _) = w.two_repo_report();
        // The fault hits the second member; the first has merged.
        w.install(IntegrateSeams {
            fault: Some(Box::new(move |repo| {
                if repo == "web" {
                    fault()
                } else {
                    SetFault::Continue
                }
            })),
            git_seams: None,
        });
        let answer = w.call("integrate", &[&report]);
        let set = result(&answer);
        assert_eq!(set["state"], "failed", "{name}: {answer}");
        assert!(
            set["reason"].as_str().unwrap().contains(reason),
            "{name}: {answer}"
        );
        assert_eq!(set["merged"], json!(["core"]), "{name}");
        assert_eq!(set["failed"], json!(["web"]), "{name}");
        let set_id = set["setId"].as_str().unwrap();
        assert_eq!(
            set["next"],
            format!("cstan integrate redo {set_id} --repo web, or cstan integrate discard {set_id} --repo web"),
            "{name}"
        );
        assert_eq!(
            w.count("SELECT count(*) FROM integrations WHERE state = 'running'"),
            0,
            "{name}"
        );
        assert_eq!(
            w.count("SELECT count(*) FROM integration_sets WHERE state = 'failed'"),
            1,
            "{name}"
        );
        // The guard is free: a new integration runs.
        w.install(IntegrateSeams::default());
        let c = commit_on(&w.repo("core"), "c.txt", "c\n");
        let next = w.report("dev-2", &[("core", &c)], None);
        let again = w.call("integrate", &[&next]);
        assert_eq!(result(&again)["state"], "merged", "{name}: {again}");
    }
}

#[test]
fn git_older_than_2_38_refuses() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    w.install(IntegrateSeams {
        fault: None,
        git_seams: Some(GitSeams {
            force_old_git: true,
            ..GitSeams::default()
        }),
    });
    let text = refusal(&w.call("integrate", &[&report]));
    assert!(
        text.contains("git 2.38 or newer is needed for workspace integrate"),
        "{text}"
    );
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
}

/// A git that answers `merge-tree` with `code` and runs everything else as git.
fn fake_git(w: &World, code: i32) -> PathBuf {
    let path = w.scratch.path().join(format!("fake-git-{code}"));
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::fs::write(
        &path,
        format!("#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = merge-tree ] && exit {code}; done\nexec {} \"$@\"\n", real.trim()),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn a_merge_tree_that_exits_1_is_not_landed_and_one_that_exits_2_refuses_naming_the_repo() {
    let w = World::new();
    let (report, _, _) = w.two_repo_report();
    let seams = |code| IntegrateSeams {
        fault: None,
        git_seams: Some(GitSeams {
            git_program: Some(fake_git(&w, code)),
            skip_version_probe: true,
            ..GitSeams::default()
        }),
    };
    w.install(seams(2));
    let text = refusal(&w.call("integrate", &[&report]));
    assert!(
        text.contains("git could not compare repo core with main (exit 2)"),
        "{text}"
    );
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
    // Exit 1 is a conflict: not landed, so both repos get a member.
    w.install(seams(1));
    let answer = w.call("integrate", &[&report]);
    assert_eq!(repos_of(result(&answer)), ["core", "web"], "{answer}");
}

// ------------------------------------------------------------------------------------------------ single repo

#[test]
fn a_single_repo_project_answers_the_same_shape_as_before() {
    let w = World::build(false);
    let root = w.root.clone();
    let branch = "feature";
    git(&root, &["checkout", "--quiet", "-b", branch, "main"]);
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "--quiet", "-m", "feat: add b"]);
    let sha = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["checkout", "--quiet", "main"]);
    let base = git(&root, &["rev-parse", "HEAD"]);
    let report = w.report("dev-1", &[], Some(&sha));
    let answer = w.call("integrate", &[&report]);
    let record = result(&answer);
    let keys: Vec<&str> = record
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "integrationId",
            "state",
            "base",
            "branch",
            "head",
            "reports",
            "conflict",
            "failure"
        ]
    );
    assert_eq!(record["state"], "merged");
    assert_eq!(record["base"], base);
    assert_eq!(record["reports"], json!([report]));
    assert_eq!(record["conflict"], Value::Null);
    assert_eq!(record["failure"], Value::Null);
    assert!(record.get("setId").is_none());
    assert_eq!(w.count("SELECT count(*) FROM integration_sets"), 0);
}

#[test]
fn decide_merge_of_a_member_is_refused_and_merges_nothing() {
    let project = common::merge::MergeProject::new();
    let id = project.integration_id.clone();
    let before = project.rev("main");
    project
        .deps
        .kernel
        .run({
            let (id, operator) = (id.clone(), project.credentials["operator"].clone());
            move |core| {
                let ctx = |core: &Core| capstan_daemon::deps::new_context(core, &operator);
                let set = core.create_integration_set(
                    &ctx(core)?,
                    &json!({"branch": "integration/x", "repos": ["core"]}),
                )?;
                core.attach_set_member(
                    &ctx(core)?,
                    &json!({"setId": set["setId"], "repo": "core", "integrationId": id}),
                )
            }
        })
        .expect("the member is attached");
    let answer = project.decide_merge("nonce-set-member");
    assert_eq!(answer["ok"], false, "{answer}");
    let text = answer.to_string();
    assert!(text.contains("belongs to set"), "{text}");
    assert_eq!(project.rev("main"), before, "nothing was merged");
}

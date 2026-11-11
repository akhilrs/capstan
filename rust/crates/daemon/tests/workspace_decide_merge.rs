#![allow(dead_code)]
//! `decide-merge` of one member of a workspace set (plan-184): the merge is made in the member's repo, into its main,
//! and the merge order of the set is a block until the earlier members are confirmed or dropped. The kernel, the daemon
//! handler and git are real.

mod common;

use capstan_config::TrustStore;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::decide_merge_seams::{install_seams, DecideMergeSeams};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::ports::ProjectGit;
use capstan_kernel::areas::snapshot_feed::content_hash;
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
        // The machine's own file stays untracked, so it is written after the repository is built.
        let local = dir.path().join("capstan.local.toml");
        std::fs::write(&local, "[remote]\nmerge = true\n").unwrap();
        std::fs::set_permissions(&local, std::fs::Permissions::from_mode(0o600)).unwrap();
        common::trusted_project(dir.path(), &trust);
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

struct Set {
    id: String,
    core: String,
    web: String,
    /// The branch of each member.
    core_branch: String,
    web_branch: String,
}

impl World {
    /// A merged set of core and web, with a passed review of the set (on its first member).
    fn merged_set(&self) -> Set {
        let (report, _, _) = self.two_repo_report();
        let answer = self.call("integrate", &[&report]);
        let set = result(&answer);
        assert_eq!(set["state"], "merged", "{answer}");
        let member = |repo: &str| -> (String, String) {
            let found = set["members"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["repo"] == repo)
                .unwrap();
            let id = found["integrationId"].as_str().unwrap().to_string();
            let branch = format!("{}-{repo}", set["branch"].as_str().unwrap());
            (id, branch)
        };
        let (core, core_branch) = member("core");
        let (web, web_branch) = member("web");
        self.pass_review(&core);
        Set {
            id: set["setId"].as_str().unwrap().to_string(),
            core,
            web,
            core_branch,
            web_branch,
        }
    }

    /// A passed review of an integration.
    fn pass_review(&self, subject: &str) {
        let conn = self.db();
        let actor: String = conn
            .query_row(
                "SELECT actor_id FROM agents WHERE agent_id = 'pm-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let sequence = self.sequence.get();
        self.sequence.set(sequence + 1);
        conn.execute(
            "INSERT INTO reviews(project_id, review_id, sequence, round, subject_integration_id, commit_sha, base_sha, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, verdict_text, state, created_at)
             VALUES ('pset', ?, ?, 1, ?, ?, ?, ?, 'pm', 'pm-1', ?, 'ok', 'passed', ?)",
            rusqlite::params![
                format!("irv-{sequence}"),
                1000 + sequence,
                subject,
                "1".repeat(40),
                "0".repeat(40),
                actor,
                actor,
                "2099-01-01T00:00:00.000Z"
            ],
        )
        .unwrap();
    }

    /// `decide-merge` of an integration as the operator.
    fn decide(&self, integration: &str, nonce: &str) -> Value {
        let argv = self.decide_argv(integration, nonce);
        self.call(
            "decide-merge",
            &argv.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }

    /// The argv of a decide-merge, with the hash of the waiting item as it is now.
    fn decide_argv(&self, integration: &str, nonce: &str) -> Vec<String> {
        let request = format!("integration:{integration}");
        let hash = {
            let request = request.clone();
            self.deps
                .kernel
                .run(move |core| content_hash(core, &request))
                .expect("the feed is read")
                .map_or_else(|| "0".repeat(12), |hash| hash[..12].to_string())
        };
        [
            request.as_str(),
            "--expect",
            &hash,
            "--nonce",
            nonce,
            "merge",
        ]
        .map(String::from)
        .to_vec()
    }

    fn state_of(&self, integration: &str) -> String {
        let id = integration.to_string();
        self.deps
            .kernel
            .run(move |core| core.integration(&id))
            .unwrap()["state"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Waits for the hand-off thread of a merge: the PM has a message that names the integration.
    fn pm_text_of(&self, integration: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let messages = self
                .deps
                .kernel
                .run(|core| core.messages_for("pm-1"))
                .unwrap();
            let found = messages
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["body"].as_str())
                .find(|body| body.contains(integration))
                .map(str::to_string);
            if let Some(text) = found {
                return text;
            }
            assert!(std::time::Instant::now() < deadline, "no message to the PM");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    /// The user lands the member's branch on its repo's main by hand.
    fn land(&self, repo: &str, branch: &str) {
        git(&self.repo(repo), &["merge", "--quiet", "--no-edit", branch]);
    }
}

#[test]
fn decide_merge_merges_one_repo_names_the_next_and_is_idempotent() {
    let w = World::new();
    let set = w.merged_set();
    let (core_before, web_before) = w.mains();
    let argv = w.decide_argv(&set.core, "nonce-core");
    let answer = w.call(
        "decide-merge",
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let merge = &result(&answer)["merge"];
    assert_eq!(merge["repo"], "core", "{answer}");
    assert_eq!(merge["setId"], set.id.as_str());
    assert_eq!(merge["nextRepo"], "web");
    assert_eq!(merge["confirm"], "confirmed");
    let (core_after, web_after) = w.mains();
    assert_ne!(core_after, core_before, "core main moved");
    assert_eq!(web_after, web_before, "web main did not");
    assert_eq!(w.state_of(&set.core), "confirmed");
    assert_eq!(w.state_of(&set.web), "merged");
    // The same nonce repeats its answer; a new one finds nothing waiting.
    let again = w.call(
        "decide-merge",
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(result(&again)["decisionId"], result(&answer)["decisionId"]);
    assert_eq!(w.mains().0, core_after, "nothing merged twice");
    let refused = w.decide(&set.core, "nonce-core-2");
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(refusal(&refused).contains("repo core: "), "{refused}");
    // The hand-off tells the PM which repo of which set.
    let text = w.pm_text_of(&set.core);
    assert!(
        text.contains(&format!("repo core of set {}", set.id)),
        "{text}"
    );
    // The last member names no next repo.
    let last = w.decide(&set.web, "nonce-web");
    assert_eq!(result(&last)["merge"]["repo"], "web", "{last}");
    assert_eq!(result(&last)["merge"]["nextRepo"], Value::Null);
}

#[test]
fn a_later_member_is_blocked_until_the_earlier_one_is_confirmed() {
    let w = World::new();
    let set = w.merged_set();
    let web_before = w.mains().1;
    let refused = w.decide(&set.web, "nonce-web");
    assert_eq!(refused["code"], "not_waiting", "{refused}");
    assert_eq!(refusal(&refused), "merge core first", "{refused}");
    assert_eq!(w.mains().1, web_before, "nothing was merged");
    assert_eq!(w.state_of(&set.web), "merged");
    // The user lands core by hand and confirms it: the block clears.
    w.land("core", &set.core_branch);
    let confirmed = w.call("integrate", &["confirm", &set.id, "--repo", "core"]);
    assert_eq!(confirmed["ok"], true, "{confirmed}");
    let answer = w.decide(&set.web, "nonce-web-2");
    assert_eq!(result(&answer)["merge"]["repo"], "web", "{answer}");
    assert_eq!(w.state_of(&set.web), "confirmed");
}

#[test]
fn a_member_confirmed_out_of_order_is_never_blocked_again() {
    let w = World::new();
    let set = w.merged_set();
    w.land("web", &set.web_branch);
    let confirmed = w.call("integrate", &["confirm", &set.id, "--repo", "web"]);
    assert_eq!(confirmed["ok"], true, "{confirmed}");
    assert!(
        confirmed["result"]["warning"]
            .as_str()
            .unwrap_or_default()
            .contains("merge_order"),
        "{confirmed}"
    );
    // Web is done; core is first anyway and merges.
    let answer = w.decide(&set.core, "nonce-core");
    assert_eq!(
        result(&answer)["merge"]["nextRepo"],
        Value::Null,
        "{answer}"
    );
    let again = w.decide(&set.web, "nonce-web");
    let text = refusal(&again);
    assert!(!text.contains("merge core first"), "{text}");
}

#[test]
fn a_merged_member_answers_as_before_even_when_an_earlier_member_is_open_again() {
    let w = World::new();
    let set = w.merged_set();
    let answer = w.decide(&set.core, "nonce-core");
    assert_eq!(answer["ok"], true, "{answer}");
    let web_argv = w.decide_argv(&set.web, "nonce-web");
    let call_web = || {
        w.call(
            "decide-merge",
            &web_argv.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    };
    let web = call_web();
    assert_eq!(result(&web)["merge"]["repo"], "web", "{web}");
    // Core is open again (a redone member has no integration yet).
    w.db()
        .execute(
            "UPDATE integration_set_members SET integration_id = NULL WHERE integration_id = ?",
            [&set.core],
        )
        .unwrap();
    let again = w.decide(&set.web, "nonce-web-2");
    let text = refusal(&again);
    assert!(text.contains("is not waiting for a merge"), "{again}");
    assert!(!text.contains("merge core first"), "{again}");
    // The same nonce repeats its answer, as before.
    let repeat = call_web();
    assert_eq!(
        result(&repeat)["decisionId"],
        result(&web)["decisionId"],
        "{repeat}"
    );
}

#[test]
fn into_is_refused_on_a_member() {
    let w = World::new();
    let set = w.merged_set();
    let mut argv = w.decide_argv(&set.core, "nonce-into");
    argv.extend(["--into".to_string(), "main".to_string()]);
    let refused = w.call(
        "decide-merge",
        &argv.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(
        refusal(&refused),
        "repo core: --into does not apply to a set member; it merges into repo core's main branch",
        "{refused}"
    );
    assert_eq!(w.state_of(&set.core), "merged");
}

#[test]
fn the_phone_card_names_the_repo_and_set_and_both_members_wait() {
    let w = World::new();
    let set = w.merged_set();
    let page = w
        .deps
        .kernel
        .run(|core| {
            capstan_kernel::areas::snapshot_feed::waiting_page(core, None, 50, Some("integration"))
        })
        .unwrap();
    let summaries: Vec<String> = page.items.iter().map(|item| item.summary.clone()).collect();
    for repo in ["core", "web"] {
        let want = format!("repo {repo} of set {}", set.id);
        assert!(
            summaries.iter().any(|s| s.contains(&want)),
            "{want} in {summaries:?}"
        );
    }
}

#[test]
fn a_dropped_earlier_member_does_not_block() {
    let w = World::new();
    let set = w.merged_set();
    let discarded = w.call("integrate", &["discard", &set.id, "--repo", "core"]);
    assert_eq!(discarded["ok"], true, "{discarded}");
    // The set is reviewed through its first live member, which is web now.
    w.pass_review(&set.web);
    let answer = w.decide(&set.web, "nonce-web");
    assert_eq!(result(&answer)["merge"]["repo"], "web", "{answer}");
}

#[test]
fn each_refusal_names_the_repo_and_leaves_main_alone() {
    let w = World::new();
    let set = w.merged_set();
    let answer = w.decide(&set.core, "nonce-core");
    assert_eq!(answer["ok"], true, "{answer}");
    let web = w.repo("web");
    let before = w.mains().1;

    // dirty_tree: a tracked file of web has a change.
    std::fs::write(web.join("f.txt"), "changed\n").unwrap();
    let dirty = w.decide(&set.web, "nonce-dirty");
    assert_eq!(dirty["code"], "dirty_tree", "{dirty}");
    assert!(refusal(&dirty).starts_with("repo web: "), "{dirty}");
    git(&web, &["checkout", "--quiet", "--", "f.txt"]);

    // wrong_branch: web is checked out on another branch than its main.
    git(&web, &["checkout", "--quiet", "-b", "elsewhere"]);
    let wrong = w.decide(&set.web, "nonce-wrong");
    assert_eq!(wrong["code"], "wrong_branch", "{wrong}");
    assert!(refusal(&wrong).starts_with("repo web: "), "{wrong}");
    git(&web, &["checkout", "--quiet", "main"]);

    // git_too_old, by the seam.
    install_seams(
        &w.deps,
        DecideMergeSeams {
            git_seams: Some(GitSeams {
                force_old_git: true,
                ..GitSeams::default()
            }),
            ..DecideMergeSeams::default()
        },
    );
    let old = w.decide(&set.web, "nonce-old");
    // The version check of the first call of git answers `git_unavailable`; `git_too_old` is the preflight's own refusal.
    assert!(
        ["git_too_old", "git_unavailable"].contains(&old["code"].as_str().unwrap()),
        "{old}"
    );
    assert!(refusal(&old).starts_with("repo web: "), "{old}");
    install_seams(&w.deps, DecideMergeSeams::default());

    // merge_conflict: web's main wrote the same file differently.
    std::fs::write(web.join("b.txt"), "other\n").unwrap();
    git(&web, &["add", "-A"]);
    git(
        &web,
        &["commit", "--quiet", "-m", "feat: write b.txt differently"],
    );
    let conflict = w.decide(&set.web, "nonce-conflict");
    assert_eq!(conflict["code"], "merge_conflict", "{conflict}");
    assert!(refusal(&conflict).starts_with("repo web: "), "{conflict}");
    assert_ne!(
        w.mains().1,
        before,
        "only the user's own commit moved web main"
    );
    assert_eq!(w.state_of(&set.web), "merged");
}

#[test]
fn a_member_of_a_project_without_repos_is_still_refused() {
    let w = World::build(false);
    let answer = w.decide("integration-1", "nonce-x");
    assert_eq!(answer["ok"], false, "{answer}");
}

#[test]
fn members_of_one_plan_package_both_stay_waiting() {
    let w = World::new();
    let set = w.merged_set();
    // One plan and package own the report both members were made from.
    let conn = w.db();
    let body = serde_json::to_string(
        &capstan_kernel::plan_body::parse_plan_body(
            r#"{"summary":"s","packages":[{"id":"a","title":"T a","owns":["src/a"],"acceptance":["x"],"estimate_hours":1}],"risks":[],"integration_order":["a"]}"#,
            8,
        )
        .unwrap(),
    )
    .unwrap();
    let long_ago = "2000-01-01T00:00:00.000Z";
    conn.execute(
        "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, created_at, updated_at)
         VALUES ('pset', 'plan-900', 900, 'T', 'normal', 'draft', 'x', 'pm-1', 0, ?1, ?1)",
        [long_ago],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
         VALUES ('pset', 'plan-900', 1, ?, ?, ?, 'pm-1', (SELECT actor_id FROM agents WHERE agent_id = 'pm-1'), ?)",
        rusqlite::params!["0".repeat(40), body, "c".repeat(64), long_ago],
    )
    .unwrap();
    conn.execute(
        "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = 'plan-900'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO plan_packages(project_id, plan_id, package_id, assignee_agent_id, assigned_at)
         VALUES ('pset', 'plan-900', 'a', 'dev-1', ?)",
        [long_ago],
    )
    .unwrap();
    let linked = w.count(
        "SELECT count(DISTINCT integration_id) FROM integration_reports WHERE integration_id IN (SELECT integration_id FROM integration_set_members)",
    );
    assert_eq!(linked, 2, "both members come from the report");
    let page = w
        .deps
        .kernel
        .run(|core| {
            capstan_kernel::areas::snapshot_feed::waiting_page(core, None, 50, Some("integration"))
        })
        .unwrap();
    assert_eq!(
        page.items.len(),
        2,
        "both members wait: {:?}",
        page.items.iter().map(|i| &i.summary).collect::<Vec<_>>()
    );
    // Both can be decided: core merges, then web (still waiting) merges.
    let core = w.decide(&set.core, "nonce-core");
    assert_eq!(result(&core)["merge"]["repo"], "core", "{core}");
    let web = w.decide(&set.web, "nonce-web");
    assert_eq!(result(&web)["merge"]["repo"], "web", "{web}");
}

//! The model of a spawn: the flag, the overrides of a requirement or a plan, the role's own chain; the fit with the host;
//! what replace keeps; what a reviewer retarget compares. Stub Herdr, real git, a real ledger in a temp dir.
mod common;

use capstan_config::RoleConfig;
use capstan_launcher::api::{
    LauncherError, LauncherService, ReplaceResult, SetupOutcome, SpawnOptions, MODEL_CHANGED,
    MODEL_HOST_MISMATCH,
};
use capstan_launcher::kernel::{Ledger, LedgerPort};
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{FrontEndSite, LauncherOptions, SetupRunner};
use common::engine::{git, git_args, Scenario, OWNER_CREDENTIAL, ROLES};
use common::ledger::context;
use common::{hash_of, test_config};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

struct Fixture {
    sc: Scenario,
    launcher: Launcher,
    calls: Mutex<Vec<Value>>,
    a: String,
    b: String,
}

const PLAN_A: &str = "plan-1";
const PLAN_B: &str = "plan-2";

fn run(sc: &Scenario, cwd: &Path, args: &[&str]) -> String {
    git(&sc.world.real_git, cwd, &git_args(args))
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"))
        .trim()
        .to_string()
}

fn sql(sc: &Scenario, statement: &'static str, params: Vec<String>) {
    sc.ledger(move |core| {
        use capstan_kernel::helpers::execute;
        let db = &core.kernel().database;
        let p: Vec<&str> = params.iter().map(String::as_str).collect();
        match p.len() {
            1 => execute(db, statement, [p[0]]),
            2 => execute(db, statement, [p[0], p[1]]),
            3 => execute(db, statement, [p[0], p[1], p[2]]),
            4 => execute(db, statement, [p[0], p[1], p[2], p[3]]),
            _ => panic!("{} parameters", p.len()),
        }
        .map(drop)
    });
}

fn body(package: &str) -> String {
    let text = format!(
        r#"{{"summary":"s","packages":[{{"id":"{package}","title":"P","owns":["src/{package}"],"acceptance":["works"],"estimate_hours":1,"interfaces":["i"]}}],"risks":[],"integration_order":["{package}"]}}"#
    );
    serde_json::to_string(&capstan_kernel::plan_body::parse_plan_body(&text, 8).unwrap()).unwrap()
}

fn base_config(sc: &Scenario) -> RoleConfig {
    let mut cfg = test_config(&sc.spec);
    let mut reviewer = cfg
        .roles
        .iter()
        .find(|r| r.name == "developer")
        .cloned()
        .expect("a developer role");
    reviewer.name = "reviewer".into();
    reviewer.kind = "Verifier".into();
    reviewer.config_hash = hash_of("reviewer");
    cfg.roles.push(reviewer);
    cfg
}

fn make_launcher(sc: &Scenario, cfg: RoleConfig) -> Launcher {
    let root = sc.project_root.clone();
    let setup: SetupRunner = Arc::new(|_, _, _| SetupOutcome::Ok);
    Launcher::new(LauncherOptions {
        ledger: Ledger::new(
            Arc::clone(&sc.world.kernel) as Arc<dyn LedgerPort>,
            OWNER_CREDENTIAL,
        ),
        adapter: sc.world.stub.clone(),
        config: cfg,
        project_root: root.to_string_lossy().into_owned(),
        project_id: "proj1".into(),
        cli_path: "/opt/capstan/cli.js".into(),
        socket_path: sc
            .state_dir
            .join("control.sock")
            .to_string_lossy()
            .into_owned(),
        credential: OWNER_CREDENTIAL.into(),
        node_path: Some("/usr/bin/node".into()),
        base_environment: HashMap::from([("PATH".to_string(), std::env::var("PATH").unwrap())]),
        git: None,
        now: None,
        log: None,
        sync_roles: None,
        run_setup: Some(setup),
        sleep: None,
        run_teardown: None,
        site: Some(FrontEndSite {
            env: HashMap::new(),
            exec_path: "/usr/bin/node".into(),
            sea: false,
            cstan: "/usr/bin/cstan".into(),
        }),
    })
}

/// `config` is applied to the resolved configuration before the launcher starts: a role's model, say.
fn fixture(name: &str, config: impl FnOnce(&mut RoleConfig)) -> Fixture {
    let sc = Scenario::new(
        name,
        json!({"maxWorkers": 8, "hostOf": {"developer2": "codex"}}),
    );
    let root = sc.project_root.clone();
    std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
    run(&sc, &root, &["add", "-A"]);
    run(
        &sc,
        &root,
        &["commit", "--quiet", "-m", "chore: ignore build"],
    );
    let a = run(&sc, &root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    run(&sc, &root, &["add", "-A"]);
    run(&sc, &root, &["commit", "--quiet", "-m", "feat: add b"]);
    let b = run(&sc, &root, &["rev-parse", "HEAD"]);

    let mut desired: Vec<Value> = ROLES
        .iter()
        .map(|(role, kind)| {
            let host = if *role == "developer2" {
                "codex"
            } else {
                "claude"
            };
            json!({"name": role, "kind": kind, "host": host, "configHash": hash_of(role)})
        })
        .collect();
    desired.push(json!({"name": "reviewer", "kind": "Verifier", "host": "claude", "configHash": hash_of("reviewer")}));
    sc.ledger(move |core| {
        let ctx = context(core, OWNER_CREDENTIAL)?;
        core.sync_role_definitions(&ctx, &Value::Array(desired))
    });

    let mut cfg = base_config(&sc);
    config(&mut cfg);
    let launcher = make_launcher(&sc, cfg);
    launcher.launch_pm().expect("the PM launches");
    let f = Fixture {
        sc,
        launcher,
        calls: Mutex::new(Vec::new()),
        a,
        b,
    };
    f.plan(PLAN_A, "a");
    f.plan(PLAN_B, "b");
    f
}

impl Fixture {
    /// An approved plan with one package.
    fn plan(&self, plan: &'static str, package: &str) {
        let stored = body(package);
        for (statement, params) in [
            (
                "INSERT INTO plans(project_id, plan_id, sequence, title, tier, state, requested_by, architect_agent_id, current_revision, approved_revision, created_at, updated_at)
                 VALUES ('proj1', ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM plans), 'T', 'normal', 'draft', 'x', 'pm-1', 0, NULL, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                vec![plan.to_string()],
            ),
            (
                "INSERT INTO plan_revisions(project_id, plan_id, revision, base_sha, body_json, body_sha, author_agent_id, author_actor_id, created_at)
                 VALUES ('proj1', ?, 1, ?, ?, ?, 'pm-1', (SELECT actor_id FROM agents WHERE agent_id = 'pm-1'), '2026-01-01T00:00:00.000Z')",
                vec![plan.to_string(), self.a.clone(), stored.clone(), "c".repeat(64)],
            ),
        ] {
            sql(&self.sc, statement, params);
        }
        sql(
            &self.sc,
            "UPDATE plans SET current_revision = 1, state = 'approved', approved_revision = 1 WHERE plan_id = ?",
            vec![plan.to_string()],
        );
    }

    /// An override row, set the way the controller would store it.
    fn put(&self, scope: &str, id: &str, kind: Option<&str>, model: &str) {
        sql(
            &self.sc,
            "INSERT INTO model_overrides(project_id, scope_type, scope_id, role_kind, model, set_by, set_at) VALUES ('proj1', ?, ?, ?, ?, 'x', '2026-01-01T00:00:00.000Z')
             ON CONFLICT(project_id, scope_type, scope_id, role_kind) DO UPDATE SET model = excluded.model",
            vec![scope.into(), id.into(), kind.unwrap_or("").into(), model.into()],
        );
    }

    fn spawn(&self, role: &str, options: SpawnOptions) -> Result<String, LauncherError> {
        self.launcher
            .spawn(role, &options)
            .map(|spawned| spawned.agent_id)
    }

    fn start_args(&self, agent: &str) -> Vec<String> {
        let mut calls = self.calls.lock().unwrap();
        calls.extend(self.sc.world.stub.take_calls());
        calls
            .iter()
            .filter(|call| call["m"] == "startAgent" && call["a"][0]["name"] == agent)
            .flat_map(|call| call["a"][0]["args"].as_array().cloned().unwrap_or_default())
            .map(|arg| arg.as_str().unwrap_or("").to_string())
            .collect()
    }

    fn model_flag(&self, agent: &str) -> Option<String> {
        let args = self.start_args(agent);
        args.iter()
            .position(|arg| arg == "--model")
            .map(|at| args[at + 1].clone())
    }

    /// (model, source, scope) recorded for the agent.
    fn row(&self, agent: &str) -> Option<(String, String, Option<String>)> {
        let agent = agent.to_string();
        self.sc.ledger(move |core| {
            Ok(core.agent_model(&agent)?.map(|v| {
                (
                    v["model"].as_str().unwrap().to_string(),
                    v["source"].as_str().unwrap().to_string(),
                    v["scopeId"].as_str().map(str::to_string),
                )
            }))
        })
    }

    /// A report row of the agent, the subject of a review.
    fn report(&self, agent: &str) -> String {
        let id = format!("report-{agent}");
        sql(
            &self.sc,
            "INSERT INTO agent_reports(project_id, report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, evidence_json, created_at)
             VALUES ('proj1', ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM agent_reports), ?, 1, (SELECT actor_id FROM agents WHERE agent_id = ?), ?, 'b', 's', 'accepted', '{}', '2026-01-01T00:00:00.000Z')",
            vec![id.clone(), agent.into(), agent.into(), self.a.clone()],
        );
        id
    }
}

/// Gives the named role a model of its own, as `[roles.<name>] model` resolves.
fn role_model(cfg: &mut RoleConfig, name: &str, model: &str) {
    let role = cfg.roles.iter_mut().find(|r| r.name == name).unwrap();
    role.model = Some(model.into());
    role.model_source = "role";
}

fn task(task: &str) -> SpawnOptions {
    SpawnOptions {
        task: Some(task.into()),
        ..Default::default()
    }
}

#[test]
fn the_flag_beats_a_plan_override_beats_the_role_model() {
    let f = fixture("model-order", |c| {
        role_model(c, "developer", "claude-role");
    });
    f.put("plan", PLAN_A, None, "claude-plan");
    let flagged = f
        .spawn(
            "developer",
            SpawnOptions {
                model: Some("claude-flag".into()),
                task: Some("plan-1/a".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(f.model_flag(&flagged).as_deref(), Some("claude-flag"));
    assert_eq!(
        f.row(&flagged),
        Some(("claude-flag".into(), "spawn".into(), None))
    );
    let planned = f.spawn("developer", task("plan-1/a")).unwrap();
    assert_eq!(f.model_flag(&planned).as_deref(), Some("claude-plan"));
    assert_eq!(
        f.row(&planned),
        Some(("claude-plan".into(), "plan".into(), Some(PLAN_A.into())))
    );
    // Plan B has no override: the role's model.
    let other = f.spawn("developer", task("plan-2/b")).unwrap();
    assert_eq!(f.model_flag(&other).as_deref(), Some("claude-role"));
    assert_eq!(
        f.row(&other),
        Some(("claude-role".into(), "role".into(), None))
    );
}

#[test]
fn a_requirement_override_beats_the_plan_and_options_plan_applies() {
    let f = fixture("model-requirement", |_| {});
    f.put("requirement", "req-1", None, "claude-req");
    f.put("plan", PLAN_A, None, "claude-plan");
    let required = f.spawn("developer", task("req-1")).unwrap();
    assert_eq!(f.model_flag(&required).as_deref(), Some("claude-req"));
    assert_eq!(
        f.row(&required),
        Some(("claude-req".into(), "task".into(), Some("req-1".into())))
    );
    // A task of a requirement names no plan, so options.plan comes next; the requirement row still wins first.
    let both = f
        .spawn(
            "developer",
            SpawnOptions {
                task: Some("req-1".into()),
                plan: Some(PLAN_A.into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(f.model_flag(&both).as_deref(), Some("claude-req"));
    let by_plan = f
        .spawn(
            "developer",
            SpawnOptions {
                plan: Some(PLAN_A.into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(f.model_flag(&by_plan).as_deref(), Some("claude-plan"));
}

#[test]
fn a_kind_row_beats_an_all_kinds_row() {
    let f = fixture("model-kind", |_| {});
    f.put("plan", PLAN_A, None, "claude-all");
    f.put("plan", PLAN_A, Some("Developer"), "claude-dev");
    let agent = f.spawn("developer", task("plan-1/a")).unwrap();
    assert_eq!(f.model_flag(&agent).as_deref(), Some("claude-dev"));
}

#[test]
fn a_spawn_without_task_plan_or_target_keeps_the_role_chain() {
    let f = fixture("model-adhoc", |c| {
        role_model(c, "supervisor", "claude-sup");
    });
    f.put("plan", PLAN_A, None, "claude-plan");
    let supervisor = f.spawn("supervisor", SpawnOptions::default()).unwrap();
    assert_eq!(f.model_flag(&supervisor).as_deref(), Some("claude-sup"));
    let adhoc = f.spawn("developer", SpawnOptions::default()).unwrap();
    assert_eq!(f.model_flag(&adhoc), None);
    assert_eq!(f.row(&adhoc), None);
}

#[test]
fn a_reviewer_takes_the_override_of_its_plan_and_codex_gets_the_flag_too() {
    let f = fixture("model-reviewer", |_| {});
    f.put("plan", PLAN_A, None, "claude-plan");
    f.put("plan", PLAN_B, None, "gpt-plan");
    let developer = f.spawn("developer", task("plan-1/a")).unwrap();
    let report = f.report(&developer);
    let reviewer = f
        .spawn(
            "reviewer",
            SpawnOptions {
                base_sha: Some(f.a.clone()),
                review_target: Some(report),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(f.model_flag(&reviewer).as_deref(), Some("claude-plan"));
    // The codex developer takes a codex model from its plan.
    let codex = f.spawn("developer2", task("plan-2/b")).unwrap();
    assert_eq!(f.model_flag(&codex).as_deref(), Some("gpt-plan"));
    assert!(f.start_args(&codex).iter().any(|arg| arg == "gpt-plan"));
}

#[test]
fn a_claude_override_on_a_codex_role_fails_and_leaves_nothing() {
    let f = fixture("model-mismatch", |_| {});
    f.put("plan", PLAN_A, None, "claude-plan");
    let agents_before = f.sc.ledger(|core| core.list_agents());
    f.sc.world.stub.take_calls();
    let error = f.spawn("developer2", task("plan-1/a")).unwrap_err();
    assert_eq!(error.code, MODEL_HOST_MISMATCH);
    assert!(
        error.message.contains("model claude-plan is a claude model and host codex is codex; use a codex model; it came from plan plan-1"),
        "{}",
        error.message
    );
    let agents_after = f.sc.ledger(|core| core.list_agents());
    assert_eq!(agents_before, agents_after);
    let calls = f.sc.world.stub.take_calls();
    assert!(
        calls
            .iter()
            .all(|call| call["m"] != "createWorktree" && call["m"] != "startAgent"),
        "{calls:?}"
    );
    let branches = run(&f.sc, &f.sc.project_root, &["branch", "--list"]);
    assert!(!branches.contains("plan-1"), "{branches}");
    // The flag is checked the same way.
    let error = f
        .spawn(
            "developer2",
            SpawnOptions {
                model: Some("opus".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, MODEL_HOST_MISMATCH);
    assert!(
        error.message.ends_with("it came from spawn"),
        "{}",
        error.message
    );
}

fn replaced_id(result: ReplaceResult) -> (String, Option<String>) {
    match result {
        ReplaceResult::Replaced(done) => (done.spawn.agent_id.clone(), done.spawn.warning.clone()),
        other => panic!("not replaced: {other:?}"),
    }
}

#[test]
fn replace_keeps_the_recorded_model_after_the_override_changed() {
    let f = fixture("model-replace", |_| {});
    f.put("plan", PLAN_A, None, "claude-one");
    let agent = f.spawn("developer", task("plan-1/a")).unwrap();
    f.put("plan", PLAN_A, None, "claude-two");
    let (successor, warning) = replaced_id(f.launcher.replace(&agent).unwrap());
    assert_eq!(warning, None);
    assert_eq!(f.model_flag(&successor).as_deref(), Some("claude-one"));
    assert_eq!(
        f.row(&successor),
        Some(("claude-one".into(), "plan".into(), Some(PLAN_A.into())))
    );
}

#[test]
fn replace_resolves_anew_when_the_recorded_model_no_longer_fits() {
    let mut f = fixture("model-replace-unfit", |_| {});
    f.put("plan", PLAN_A, Some("Developer"), "gpt-old");
    let agent = f.spawn("developer2", task("plan-1/a")).unwrap();
    assert_eq!(f.row(&agent).unwrap().0, "gpt-old");
    // The role moved to a claude host since, and the plan now names a claude model for developers.
    let mut cfg = base_config(&f.sc);
    cfg.roles
        .iter_mut()
        .find(|r| r.name == "developer2")
        .unwrap()
        .host = "claude".into();
    f.launcher = make_launcher(&f.sc, cfg);
    f.sc.ledger(|core| {
        let ctx = context(core, OWNER_CREDENTIAL)?;
        core.sync_role_definitions(
            &ctx,
            &json!([{"name": "developer2", "kind": "Developer", "host": "claude", "configHash": hash_of("developer2")}]),
        )
    });
    f.put("plan", PLAN_A, Some("Developer"), "claude-new");
    let (successor, warning) = replaced_id(f.launcher.replace(&agent).unwrap());
    assert_eq!(
        warning.as_deref(),
        Some("model gpt-old no longer fits host claude; the replacement uses claude-new (plan)")
    );
    assert_eq!(f.model_flag(&successor).as_deref(), Some("claude-new"));
    assert_eq!(
        f.row(&successor),
        Some(("claude-new".into(), "plan".into(), Some(PLAN_A.into())))
    );
}

/// A reviewer spawned for the report of a developer on `plan`, at the first commit.
fn reviewer_for(f: &Fixture, plan_task: &str) -> (String, String) {
    let developer = f.spawn("developer", task(plan_task)).unwrap();
    let report = f.report(&developer);
    let reviewer = f
        .spawn(
            "reviewer",
            SpawnOptions {
                base_sha: Some(f.a.clone()),
                review_target: Some(report.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    (reviewer, report)
}

#[test]
fn retarget_fails_with_model_changed_across_plans_and_passes_within_one() {
    let f = fixture("model-retarget", |_| {});
    f.put("plan", PLAN_A, None, "claude-a");
    f.put("plan", PLAN_B, None, "claude-b");
    let (reviewer, report_a) = reviewer_for(&f, "plan-1/a");
    assert_eq!(f.row(&reviewer).unwrap().0, "claude-a");
    let developer_b = f.spawn("developer", task("plan-2/b")).unwrap();
    let report_b = f.report(&developer_b);
    let error = f
        .launcher
        .retarget_review(&reviewer, &f.b, &report_b)
        .unwrap_err();
    assert_eq!(error.code, MODEL_CHANGED);
    // Nothing moved.
    let path = f.sc.ledger({
        let reviewer = reviewer.clone();
        move |core| {
            Ok(core
                .agent_panes(OWNER_CREDENTIAL)?
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["agentId"] == reviewer.as_str())
                .map(|row| row["worktreePath"].as_str().unwrap().to_string())
                .unwrap())
        }
    });
    assert_eq!(run(&f.sc, Path::new(&path), &["rev-parse", "HEAD"]), f.a);
    f.launcher
        .retarget_review(&reviewer, &f.b, &report_a)
        .expect("the same plan passes");
    assert_eq!(run(&f.sc, Path::new(&path), &["rev-parse", "HEAD"]), f.b);
}

#[test]
fn retarget_passes_without_a_row_and_without_any_model() {
    let f = fixture("model-retarget-none", |_| {});
    let (reviewer, report) = reviewer_for(&f, "plan-1/a");
    assert_eq!(f.row(&reviewer), None);
    f.launcher
        .retarget_review(&reviewer, &f.b, &report)
        .expect("no row and no model: the config chain matches");
}

#[test]
fn retarget_compares_a_row_less_agent_with_the_config_chain() {
    let f = fixture("model-retarget-chain", |c| {
        role_model(c, "reviewer", "claude-role");
    });
    let (reviewer, report) = reviewer_for(&f, "plan-1/a");
    assert_eq!(f.row(&reviewer).unwrap().1, "role");
    sql(
        &f.sc,
        "DELETE FROM agent_models WHERE agent_id = ?",
        vec![reviewer.clone()],
    );
    f.launcher
        .retarget_review(&reviewer, &f.b, &report)
        .expect("the role's model matches");
    // A plan override for the subject changes what the review wants.
    f.put("plan", PLAN_A, None, "claude-other");
    let error = f
        .launcher
        .retarget_review(&reviewer, &f.a, &report)
        .unwrap_err();
    assert_eq!(error.code, MODEL_CHANGED);
}

#[test]
fn retarget_passes_when_a_plan_override_names_the_model_the_reviewer_runs() {
    let f = fixture("model-retarget-same", |c| {
        role_model(c, "reviewer", "claude-same");
    });
    let (reviewer, _) = reviewer_for(&f, "plan-1/a");
    assert_eq!(f.row(&reviewer).unwrap().1, "role");
    let developer_b = f.spawn("developer", task("plan-2/b")).unwrap();
    let report_b = f.report(&developer_b);
    f.put("plan", PLAN_B, None, "claude-same");
    f.launcher
        .retarget_review(&reviewer, &f.b, &report_b)
        .expect("the same bytes match whatever the source");
}

#[test]
fn overrides_never_touch_capstan_toml() {
    let f = fixture("model-toml", |_| {});
    let path = f.sc.project_root.join("capstan.toml");
    std::fs::write(&path, "schema_version = 1\n[nexora]\ntrack = \"never\"\n").unwrap();
    let before = std::fs::read(&path).unwrap();
    let digest = capstan_config::sha256(&String::from_utf8(before.clone()).unwrap());
    f.sc.ledger(|core| {
        let set = |model: &str| {
            core.set_model_override(
                &context(core, OWNER_CREDENTIAL)?,
                &json!({"scopeType": "plan", "scopeId": PLAN_A, "roleKind": null, "model": model}),
            )
        };
        set("claude-a")?;
        set("claude-b")?;
        core.clear_model_override(
            &context(core, OWNER_CREDENTIAL)?,
            &json!({"scopeType": "plan", "scopeId": PLAN_A, "roleKind": null}),
        )?;
        Ok(())
    });
    let agent = f.spawn("developer", task("plan-1/a")).unwrap();
    assert_eq!(f.model_flag(&agent), None);
    let after = std::fs::read(&path).unwrap();
    assert_eq!(before, after);
    assert_eq!(
        digest,
        capstan_config::sha256(&String::from_utf8(after.clone()).unwrap())
    );
}

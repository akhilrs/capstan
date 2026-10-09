//! Replays the integration scenarios exported from Node (test/kernel-integrate-export.ts, files in
//! tests/parity-integrate) on the Rust pipeline: the same scratch git repositories, the same controller operations, and
//! the pipeline of src/integrate.rs in place of src/integration.ts. Every step's result or error, the logs, the
//! repository (every branch with its commit id, tree id, parents and message), the counters and the ledger tables must
//! equal what Node recorded.
//!
//! Git runs only in directories this test creates under the system temporary directory, with an empty global
//! configuration, no system configuration, an isolated HOME, a fixed identity and fixed commit dates.

mod common;

use capstan_kernel::env::Env;
use capstan_kernel::integrate::{
    check_commit_message, integrate, integration_branch_name, parse_commit_subject,
    recover_integrations, settle_integration, slugify, squash_message, with_suffix, GitRepo,
    IntegrateError, IntegrationDeps, IntegrationState, Scope,
};
use capstan_kernel::types::{InitialProject, MutationContext};
use capstan_kernel::{Core, KernelError, KernelOptions, SeededEnv};
use capstan_ledger::{open_database, OpenOptions};
use common::replay::{dump_tables, first_difference};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

fn parity_integrate_dir() -> PathBuf {
    common::crate_dir().join("tests").join("parity-integrate")
}

fn strict() -> bool {
    std::env::var("CAPSTAN_KERNEL_PARITY_STRICT").is_ok_and(|v| v == "1")
}

struct SharedEnv(Rc<SeededEnv>);

impl Env for SharedEnv {
    fn now(&self) -> i64 {
        self.0.now()
    }
    fn uuid(&self) -> String {
        self.0.uuid()
    }
    fn random_bytes(&self, n: usize) -> Vec<u8> {
        self.0.random_bytes(n)
    }
}

/// A scratch repository the scenario builds and inspects, with an environment that cannot see the user's git setup.
struct Scratch {
    root: PathBuf,
    env: Vec<(String, String)>,
}

impl Scratch {
    fn new(base: &Path, pinned: Vec<(String, String)>) -> Self {
        let root = base.join("repo");
        let home = base.join("home");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&home).unwrap();
        assert!(
            root.starts_with(std::env::temp_dir()),
            "a scratch repository lives under the temporary directory"
        );
        let mut env = vec![
            (
                "PATH".to_string(),
                std::env::var("PATH").unwrap_or_default(),
            ),
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("LC_ALL".into(), "C".into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_AUTHOR_NAME".into(), "scratch".into()),
            ("GIT_AUTHOR_EMAIL".into(), "scratch@localhost".into()),
            ("GIT_COMMITTER_NAME".into(), "scratch".into()),
            ("GIT_COMMITTER_EMAIL".into(), "scratch@localhost".into()),
        ];
        env.extend(pinned);
        Self { root, env }
    }

    fn run(&self, args: &[&str]) -> (i32, String) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("git runs");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    fn must(&self, args: &[&str]) -> String {
        let (code, out) = self.run(args);
        assert_eq!(code, 0, "git {args:?} failed");
        out
    }

    fn commit(&self, spec: &Value) -> String {
        let branch = spec["branch"].as_str().unwrap();
        if let Some(from) = spec["from"].as_str() {
            self.must(&["checkout", "-q", "-B", branch, from]);
        }
        for (name, content) in spec["files"].as_object().unwrap() {
            let file = self.root.join(name);
            match content.as_str() {
                None => {
                    let _ = std::fs::remove_file(&file);
                }
                Some(text) => {
                    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                    std::fs::write(&file, text).unwrap();
                }
            }
        }
        self.must(&["add", "-A"]);
        self.must(&[
            "commit",
            "-q",
            "--no-verify",
            "--no-gpg-sign",
            "--allow-empty",
            "-m",
            spec["message"].as_str().unwrap(),
        ]);
        let sha = self.must(&["rev-parse", "HEAD"]).trim().to_string();
        if branch != "main" {
            self.must(&["checkout", "-q", "main"]);
        }
        sha
    }

    fn snapshot(&self) -> Value {
        let refs: Vec<String> = self
            .must(&["for-each-ref", "--format=%(refname)", "refs/heads"])
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        let entries: Vec<Value> = refs
            .iter()
            .map(|reference| {
                let raw = self.must(&["cat-file", "commit", reference]);
                let split = raw.find("\n\n").unwrap();
                let header: Vec<&str> = raw[..split].split('\n').collect();
                json!({
                    "ref": reference,
                    "sha": self.must(&["rev-parse", reference]).trim(),
                    "tree": header.iter().find_map(|l| l.strip_prefix("tree ")).unwrap_or(""),
                    "parents": header.iter().filter_map(|l| l.strip_prefix("parent ")).collect::<Vec<_>>(),
                    "message": &raw[split + 2..],
                })
            })
            .collect();
        json!({
            "head": self.must(&["symbolic-ref", "-q", "HEAD"]).trim(),
            "refs": entries,
        })
    }
}

fn outcome_of_kernel(result: &Result<Value, KernelError>) -> Value {
    match result {
        Ok(value) => json!({"result": value}),
        Err(error) => json!({"error": error.name(), "message": error.message()}),
    }
}

fn outcome_of_pipeline(result: &Result<Value, IntegrateError>) -> Value {
    match result {
        Ok(value) => json!({"result": value}),
        Err(IntegrateError::Integration { code, message }) => {
            json!({"error": "IntegrationError", "message": message, "code": code})
        }
        Err(error) => json!({"error": error.name(), "message": error.message()}),
    }
}

fn recorded_outcome(step: &Value) -> Value {
    match step.get("error") {
        Some(error) => {
            let mut out = json!({"error": error, "message": step["message"]});
            if let Some(code) = step.get("code") {
                out["code"] = code.clone();
            }
            out
        }
        None => json!({"result": step["result"]}),
    }
}

#[derive(Debug, PartialEq)]
enum Status {
    Passed,
    Pending(String),
    Failed(String),
}

fn replay_scenario(scenario: &Value) -> Status {
    let seed = scenario["seed"].as_str().unwrap_or("");
    let requires = scenario["requires"]
        .as_array()
        .is_some_and(|r| !r.is_empty());
    let state_dir = common::private_tempdir();
    let scratch_dir = tempfile::tempdir().expect("a scratch directory");
    let pinned: Vec<(String, String)> = scenario["gitEnv"]
        .as_object()
        .map(|env| {
            env.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let scratch = Scratch::new(scratch_dir.path(), pinned.clone());
    let git = GitRepo::new(scratch.root.clone()).with_env(pinned);
    let project: InitialProject = serde_json::from_value(scenario["project"].clone()).unwrap();
    let credential = project.owner_credential.clone();
    let env = Rc::new(SeededEnv::new(seed));
    let state = IntegrationState::new();
    let counter = Cell::new(0u64);
    let logs: RefCell<Vec<Value>> = RefCell::new(Vec::new());
    let mut core: Option<Core> = None;
    let mut status = Status::Passed;

    let steps = scenario["steps"].as_array().cloned().unwrap_or_default();
    'steps: for (index, step) in steps.iter().enumerate() {
        let op = step["op"].as_str().unwrap_or("");
        let args: Vec<Value> = step["args"].as_array().cloned().unwrap_or_default();
        logs.borrow_mut().clear();
        let fail = |detail: String| Status::Failed(format!("step {index} ({op}): {detail}"));
        let actual: Value = match op {
            "open" => {
                let path = state_dir.path().to_path_buf();
                open_database(&path.join("controller.sqlite"), &OpenOptions::default())
                    .unwrap()
                    .close()
                    .unwrap();
                let options = KernelOptions {
                    workspace_root: None,
                    runtime_workspace_path: None,
                    keep_migration_backups: None,
                };
                match Core::open(&path, &project, &options, Box::new(SharedEnv(env.clone()))) {
                    Ok(opened) => {
                        core = Some(opened);
                        json!({"result": Value::Null})
                    }
                    Err(error) => outcome_of_kernel(&Err(error)),
                }
            }
            "dump" => json!({"result": Value::Null}),
            "git.init" => {
                scratch.must(&["init", "-q", "-b", "main"]);
                json!({"result": Value::Null})
            }
            "git.commit" => json!({"result": scratch.commit(&args[0])}),
            "git.run" => {
                let argv: Vec<String> = args[0]["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| a.as_str().unwrap().to_string())
                    .collect();
                let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
                json!({"result": {"code": scratch.run(&refs).0}})
            }
            "git.integrate" | "git.settle" | "git.recover" => {
                let Some(core) = core.as_ref() else {
                    return fail("no controller is open".into());
                };
                let context = |cred: &str| -> MutationContext {
                    counter.set(counter.get() + 1);
                    let n = counter.get();
                    MutationContext {
                        credential: cred.to_string(),
                        request_id: format!("int-req-{n}"),
                        idempotency_key: format!("int-idem-{n}"),
                        expected_version: core.state_version().unwrap(),
                        input_revision: core.input_revision().unwrap(),
                    }
                };
                let log = |event: &str, details: Value| {
                    logs.borrow_mut()
                        .push(json!({"event": event, "details": details}));
                };
                let deps = IntegrationDeps {
                    kernel: core.kernel(),
                    git: &git,
                    state: &state,
                    credential: &credential,
                    context: &context,
                    log: &log,
                };
                let arg = &args[0];
                let result = match op {
                    "git.integrate" => {
                        let ids: Vec<String> = arg["reportIds"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_str().unwrap().to_string())
                            .collect();
                        integrate(&deps, &ids, arg["requestedBy"].as_str().unwrap())
                    }
                    "git.settle" => settle_integration(
                        &deps,
                        arg["integrationId"].as_str().unwrap(),
                        arg["outcome"].as_str().unwrap(),
                    ),
                    _ => recover_integrations(
                        &deps,
                        if arg["scope"] == "pending" {
                            Scope::Pending
                        } else {
                            Scope::All
                        },
                    )
                    .map(|()| Value::Null),
                };
                if let Err(IntegrateError::Kernel(error)) = &result {
                    if error.is_unported() {
                        let detail = format!("step {index} ({op}) reached {}", error.message());
                        return if requires {
                            Status::Pending(detail)
                        } else {
                            Status::Failed(format!("{detail} but the scenario does not require it"))
                        };
                    }
                }
                outcome_of_pipeline(&result)
            }
            _ => {
                let Some(core) = core.as_ref() else {
                    return fail("no controller is open".into());
                };
                let mut call: Vec<Value> = Vec::new();
                if let Some(context) = step.get("context") {
                    call.push(context.clone());
                }
                call.extend(args.iter().cloned());
                let result = core.dispatch(op, &call);
                if let Err(error) = &result {
                    if error.is_unported() {
                        let detail = format!("step {index} ({op}) reached {}", error.message());
                        return if requires {
                            Status::Pending(detail)
                        } else {
                            Status::Failed(format!("{detail} but the scenario does not require it"))
                        };
                    }
                }
                outcome_of_kernel(&result)
            }
        };
        if let Some(diff) = first_difference(&recorded_outcome(step), &actual, "outcome") {
            let shown = actual.to_string();
            status = fail(format!(
                "{diff} (got {})",
                shown.chars().take(300).collect::<String>()
            ));
            break 'steps;
        }
        if let Some(expected) = step.get("logs") {
            let actual = Value::Array(logs.borrow().clone());
            if let Some(diff) = first_difference(expected, &actual, "logs") {
                status = fail(diff);
                break 'steps;
            }
        }
        if let Some(expected) = step.get("repo") {
            if let Some(diff) = first_difference(expected, &scratch.snapshot(), "repo") {
                status = fail(diff);
                break 'steps;
            }
        }
        if let (Some(version), Some(core)) = (step.get("stateVersion"), core.as_ref()) {
            let actual = json!({
                "stateVersion": core.state_version().unwrap(),
                "inputRevision": core.input_revision().unwrap(),
            });
            let expected = json!({"stateVersion": version, "inputRevision": step["inputRevision"]});
            if let Some(diff) = first_difference(&expected, &actual, "counters") {
                status = fail(diff);
                break 'steps;
            }
        }
        if let Some(tables) = step.get("tables") {
            let actual = dump_tables(&state_dir.path().join("controller.sqlite"));
            if let Some(diff) = first_difference(tables, &actual, "tables") {
                status = fail(diff);
                break 'steps;
            }
        }
    }
    if status == Status::Passed {
        let actual = dump_tables(&state_dir.path().join("controller.sqlite"));
        if let Some(diff) = first_difference(&scenario["tables"], &actual, "tables") {
            status = Status::Failed(format!("final dump: {diff}"));
        }
    }
    if let Some(core) = &core {
        core.close();
    }
    status
}

#[test]
fn every_integration_scenario_replays() {
    let file = parity_integrate_dir().join("scenarios.json");
    let scenarios = common::read_json(&file);
    let mut failures = Vec::new();
    let mut pending = Vec::new();
    let mut passed = 0;
    for scenario in scenarios["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap_or("?");
        match replay_scenario(scenario) {
            Status::Passed => passed += 1,
            Status::Pending(detail) => pending.push(format!("{name}: {detail}")),
            Status::Failed(detail) => failures.push(format!("{name}: {detail}")),
        }
    }
    eprintln!(
        "integration scenarios: {passed} passed, {} pending",
        pending.len()
    );
    for line in &pending {
        eprintln!("  pending {line}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        !strict() || pending.is_empty(),
        "pending scenarios under CAPSTAN_KERNEL_PARITY_STRICT=1:\n{}",
        pending.join("\n")
    );
    assert!(passed + pending.len() > 0, "the file holds no scenario");
}

#[test]
fn squash_messages_are_node_s_text() {
    let file = parity_integrate_dir().join("squash.json");
    let cases = common::read_json(&file);
    let mut failures = Vec::new();
    for case in cases["cases"].as_array().unwrap() {
        let subjects: HashMap<String, String> = case["subjects"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
        let (subject, body) = squash_message(&case["info"], &subjects);
        let actual = json!({"subject": subject, "body": body});
        let expected = json!({"subject": case["subject"], "body": case["body"]});
        if let Some(diff) = first_difference(&expected, &actual, "squash") {
            failures.push(format!("{}: {diff}", case["name"].as_str().unwrap_or("?")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn naming_conventions_are_node_s() {
    let file = parity_integrate_dir().join("conventions.json");
    let cases = common::read_json(&file);
    let mut failures: Vec<String> = Vec::new();
    let mut check = |what: &str, expected: Value, actual: Value| {
        if let Some(diff) = first_difference(&expected, &actual, what) {
            failures.push(diff);
        }
    };
    for case in cases["slugs"].as_array().unwrap() {
        let title = case["title"].as_str().unwrap();
        let max = case["max"].as_u64().unwrap() as usize;
        check(
            &format!("slugify({title:?}, {max})"),
            case["slug"].clone(),
            json!(slugify(title, max)),
        );
    }
    for case in cases["branches"].as_array().unwrap() {
        let id = case["integrationId"].as_str().unwrap();
        let title = case["planTitle"].as_str();
        check(
            &format!("integrationBranchName({id:?}, {title:?})"),
            case["name"].clone(),
            json!(integration_branch_name(id, title)),
        );
    }
    for case in cases["suffixes"].as_array().unwrap() {
        let branch = case["branch"].as_str().unwrap();
        let n = case["n"].as_u64().unwrap() as usize;
        check(
            &format!("withSuffix({branch:?}, {n})"),
            case["name"].clone(),
            json!(with_suffix(branch, n)),
        );
    }
    for case in cases["subjects"].as_array().unwrap() {
        let subject = case["subject"].as_str().unwrap();
        let actual = match parse_commit_subject(subject) {
            Some(p) => json!({
                "type": p.kind,
                "scope": p.scope,
                "breaking": p.breaking,
                "description": p.description,
            }),
            None => Value::Null,
        };
        check(
            &format!("parseCommitSubject({subject:?})"),
            case["parsed"].clone(),
            actual,
        );
    }
    for case in cases["messages"].as_array().unwrap() {
        let message = case["message"].as_str().unwrap();
        let parents = case["parents"].as_u64().unwrap() as usize;
        let rules: Vec<&str> = check_commit_message(message, parents)
            .into_iter()
            .map(|(rule, _)| rule)
            .collect();
        check(
            &format!("checkCommitMessage({message:?}, {parents})"),
            case["rules"].clone(),
            json!(rules),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A scratch repository with a base commit and one commit on each of the branches `a` and `c`.
fn small_repo(dir: &Path) -> (Scratch, String, String, String) {
    let scratch = Scratch::new(dir, Vec::new());
    scratch.must(&["init", "-q", "-b", "main"]);
    let base = scratch.commit(&json!({
        "branch": "main", "files": {"shared.txt": "base\n"}, "message": "base",
    }));
    let a = scratch.commit(&json!({
        "branch": "a", "from": base, "files": {"shared.txt": "from a\n"}, "message": "a",
    }));
    let c = scratch.commit(&json!({
        "branch": "c", "from": base, "files": {"own.txt": "own\n"}, "message": "c",
    }));
    (scratch, base, a, c)
}

#[test]
fn repository_hooks_and_settings_do_not_run_during_a_merge() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (scratch, base, a, c) = small_repo(dir.path());
    let marker = scratch.root.join("hook-ran");
    let script = format!("#!/bin/sh\ntouch {}\nexit 1\n", marker.display());
    for hook in ["post-checkout", "pre-merge-commit", "commit-msg"] {
        let file = scratch.root.join(".git").join("hooks").join(hook);
        std::fs::write(&file, &script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let program = dir.path().join("program");
    std::fs::write(&program, &script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let program = program.to_string_lossy().into_owned();
    for (key, value) in [
        ("core.fsmonitor", program.as_str()),
        ("commit.gpgSign", "true"),
        ("gpg.program", program.as_str()),
        ("rerere.enabled", "true"),
        ("rerere.autoUpdate", "true"),
    ] {
        scratch.must(&["config", key, value]);
    }
    let git = GitRepo::new(scratch.root.clone());
    let merges = vec![("r-a".to_string(), a), ("r-c".to_string(), c)];
    let result = git
        .merge_into_branch(
            &base,
            "capstan/integration/hooks",
            "chore: combined",
            "body",
            &merges,
        )
        .unwrap();
    // `a` and `c` touch different files, so they merge.
    assert!(
        matches!(
            result,
            capstan_kernel::integrate::MergeResult::Merged { .. }
        ),
        "{result:?}"
    );
    assert!(!marker.exists(), "a hook or configured program ran");
}

#[test]
fn a_missing_commit_a_contained_report_and_a_taken_branch_fail_cleanly() {
    use capstan_kernel::integrate::MergeResult;
    let dir = tempfile::tempdir().unwrap();
    let (scratch, base, _a, c) = small_repo(dir.path());
    let git = GitRepo::new(scratch.root.clone());
    let failed = |reason: &str| MergeResult::Failed {
        reason: reason.into(),
    };
    let run = |branch: &str, merges: Vec<(String, String)>| {
        git.merge_into_branch(&base, branch, "chore: x", "body", &merges)
            .unwrap()
    };
    assert_eq!(
        run(
            "capstan/integration/missing",
            vec![("r-x".into(), "9".repeat(40))]
        ),
        failed("the commit of report r-x does not exist")
    );
    assert_eq!(git.branch_tip("capstan/integration/missing").unwrap(), None);
    assert_eq!(
        run(
            "capstan/integration/inbase",
            vec![("r-base".into(), base.clone())]
        ),
        failed("every report is already contained in the base commit")
    );
    assert_eq!(
        run(
            "capstan/integration/not sha",
            vec![("r-c".into(), c.clone())]
        ),
        failed("the branch name is not a valid ref")
    );
    assert_eq!(
        run(
            "capstan/integration/short",
            vec![("r-c".into(), "abc".into())]
        ),
        failed("a commit id is not a full sha1")
    );
    scratch.must(&["branch", "capstan/integration/taken", &base]);
    assert_eq!(
        run("capstan/integration/taken", vec![("r-c".into(), c)]),
        failed("git could not create the integration branch")
    );
    assert_eq!(
        git.branch_tip("capstan/integration/taken").unwrap(),
        Some(base),
        "a branch that already existed is left alone"
    );
}

#[test]
fn git_that_cannot_run_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let git = GitRepo::new(dir.path().join("no-such-repository"));
    assert!(git.head_commit().is_err());
    assert!(git.commit_exists("not a sha").is_err());
    assert!(git.is_in_head("abc").is_err());
}

//! The replay engine of the parity sequences: runs the steps of a sequence exported from Node by
//! test/launcher-parity-export.ts on the Rust launcher over `StubHerdr`, real git in a scratch repository and a real
//! `Core` with the same seeded clock and randomness, and compares everything the exporter recorded for each step: the
//! result or error, the calls to the adapter, the git commands, the log events, the setup and teardown commands, the
//! state of git and the change in the ledger.

use super::ledger::{context, diff_tables, dump_tables, KernelThread};
use super::stub::StubHerdr;
use super::{first_difference, hash_of, normal, test_config};
use capstan_launcher::api::{
    LauncherError, LauncherResult, LauncherService, SetupOutcome, SpawnOptions,
};
use capstan_launcher::kernel::{Ledger, LedgerPort};
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{FrontEndSite, LauncherOptions};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const OWNER_CREDENTIAL: &str = "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";

pub fn project() -> Value {
    json!({
        "projectId": "proj1",
        "name": "Parity Project",
        "ownerCredential": OWNER_CREDENTIAL,
        "initialInputs": [
            {"kind": "project_config", "content": {"name": "parity"}},
            {"kind": "task_brief", "content": {"objective": "prove parity"}},
            {"kind": "acceptance_criteria", "content": {"criteria": ["it matches"]}},
            {"kind": "policy", "content": {"review": "required"}},
            {"kind": "plan", "content": {"steps": ["one", "two"]}},
        ],
    })
}

/// The environment of the harness's own git commands (`gitEnvironment` of the exporter).
pub fn git_environment() -> Vec<(String, String)> {
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into());
    [
        ("PATH", path.as_str()),
        ("LC_ALL", "C"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_AUTHOR_NAME", "Parity"),
        ("GIT_AUTHOR_EMAIL", "parity@example.com"),
        ("GIT_COMMITTER_NAME", "Parity"),
        ("GIT_COMMITTER_EMAIL", "parity@example.com"),
        ("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z"),
        ("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

pub fn git(real_git: &str, cwd: &Path, args: &[String]) -> Result<String, String> {
    let output = std::process::Command::new(real_git)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(git_environment())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn git_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// `"$name.path"` strings replaced by the named result.
fn resolve(value: &Value, bindings: &HashMap<String, Value>) -> Result<Value, String> {
    match value {
        Value::String(text)
            if text.starts_with('$')
                && text[1..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic()) =>
        {
            let mut parts = text[1..].split('.');
            let name = parts.next().unwrap_or("");
            let mut current = bindings
                .get(name)
                .ok_or_else(|| format!("unknown reference {text}"))?;
            for part in parts {
                current = current
                    .get(part)
                    .ok_or_else(|| format!("{text} does not resolve"))?;
            }
            Ok(current.clone())
        }
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|v| resolve(v, bindings))
                .collect::<Result<_, _>>()?,
        )),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, v) in map {
                out.insert(key.clone(), resolve(v, bindings)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

fn text(step: &Value, key: &str) -> String {
    step[key].as_str().unwrap_or("").to_string()
}

fn opt(options: &Value, key: &str) -> Option<String> {
    options.get(key).and_then(Value::as_str).map(str::to_string)
}

fn spawn_options(options: &Value) -> SpawnOptions {
    SpawnOptions {
        base_sha: opt(options, "baseSha"),
        seed: opt(options, "seed"),
        task: opt(options, "task"),
        type_: opt(options, "type"),
        title: opt(options, "title"),
        review_target: opt(options, "reviewTarget"),
        branch: opt(options, "branch"),
        record_task_ref: opt(options, "recordTaskRef"),
        record_task_title: opt(options, "recordTaskTitle"),
    }
}

fn outcome_of(value: &Value) -> SetupOutcome {
    match value["status"].as_str() {
        Some("failed") => SetupOutcome::Failed {
            exit_code: value["exitCode"].as_i64().map(|c| c as i32),
            output: value["output"].as_str().unwrap_or("").to_string(),
        },
        Some("timeout") => SetupOutcome::Timeout,
        _ => SetupOutcome::Ok,
    }
}

fn plain<T>(result: LauncherResult<T>, to_value: impl FnOnce(T) -> Value) -> Result<Value, Value> {
    match result {
        Ok(value) => Ok(to_value(value)),
        Err(LauncherError { code, message }) => {
            Err(json!({"code": code, "message": message, "name": "LauncherError"}))
        }
    }
}

/// What the launcher calls to sync the roles on demand.
pub type SyncFn = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

pub struct World {
    pub scratch: String,
    pub project_root: PathBuf,
    pub real_git: String,
    pub git_log: PathBuf,
    pub git_offset: usize,
    pub stub: Arc<StubHerdr>,
    pub kernel: Arc<KernelThread>,
    pub events: Arc<Mutex<Vec<Value>>>,
    pub sleeps: Arc<Mutex<Vec<u64>>>,
    pub commands: Arc<Mutex<Vec<Value>>>,
    pub setup_outcomes: Arc<Mutex<VecDeque<SetupOutcome>>>,
    pub teardown_outcomes: Arc<Mutex<VecDeque<SetupOutcome>>>,
    /// What the launcher calls to sync the roles on demand (`LauncherOptions::sync_roles`); none unless a test sets it.
    pub sync_roles: Mutex<Option<SyncFn>>,
    /// A fake clock (milliseconds) the launcher reads instead of the system's, when a test sets one; a teardown moves it on
    /// by `teardown_advance_ms`.
    pub clock: Mutex<Option<Arc<std::sync::atomic::AtomicI64>>>,
    pub teardown_advance_ms: std::sync::atomic::AtomicI64,
}

impl World {
    pub fn make_launcher(&self, spec: &Value, state_dir: &Path) -> Launcher {
        self.make_launcher_at(spec, state_dir, &self.project_root)
    }

    /// The same launcher for another project root (a path the launcher refuses, say).
    pub fn make_launcher_at(
        &self,
        spec: &Value,
        state_dir: &Path,
        project_root: &Path,
    ) -> Launcher {
        let scratch = self.scratch.clone();
        let events = Arc::clone(&self.events);
        let sleeps = Arc::clone(&self.sleeps);
        let commands = Arc::clone(&self.commands);
        let setup = Arc::clone(&self.setup_outcomes);
        let teardown = Arc::clone(&self.teardown_outcomes);
        let mut base_environment: HashMap<String, String> = [
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/x"),
            ("LANG", "C"),
            ("SECRET", "no"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        if let Some(extra) = spec["base"].as_object() {
            for (k, v) in extra {
                base_environment.insert(k.clone(), v.as_str().unwrap_or("").to_string());
            }
        }
        let log_scratch = scratch.clone();
        let setup_scratch = scratch.clone();
        let setup_commands = Arc::clone(&commands);
        let teardown_scratch = scratch.clone();
        let clock = self.clock.lock().unwrap().clone();
        let advance = self
            .teardown_advance_ms
            .load(std::sync::atomic::Ordering::SeqCst);
        let teardown_clock = clock.clone();
        Launcher::new(LauncherOptions {
            ledger: Ledger::new(
                Arc::clone(&self.kernel) as Arc<dyn LedgerPort>,
                OWNER_CREDENTIAL,
            ),
            adapter: self.stub.clone(),
            config: test_config(spec),
            project_root: project_root.to_string_lossy().into_owned(),
            cli_path: "/opt/capstan/cli.js".into(),
            socket_path: state_dir
                .join("control.sock")
                .to_string_lossy()
                .into_owned(),
            credential: OWNER_CREDENTIAL.into(),
            node_path: Some("/usr/bin/node".into()),
            base_environment,
            git: None,
            now: clock.map(|clock| {
                Arc::new(move || clock.load(std::sync::atomic::Ordering::SeqCst))
                    as capstan_launcher::shared::Clock
            }),
            log: Some(Arc::new(move |event, details| {
                events.lock().unwrap().push(normal(
                    &json!({"event": event, "details": details}),
                    &log_scratch,
                ));
            })),
            sync_roles: self.sync_roles.lock().unwrap().clone(),
            run_setup: Some(Arc::new(move |command, cwd, timeout_ms| {
                setup_commands.lock().unwrap().push(normal(
                    &json!({"kind": "setup", "command": command, "cwd": cwd, "timeoutMs": timeout_ms}),
                    &setup_scratch,
                ));
                setup
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(SetupOutcome::Ok)
            })),
            sleep: Some(Arc::new(move |ms| sleeps.lock().unwrap().push(ms))),
            run_teardown: Some(Arc::new(move |command, cwd, timeout_ms, environment| {
                if let Some(clock) = &teardown_clock {
                    clock.fetch_add(advance, std::sync::atomic::Ordering::SeqCst);
                }
                commands.lock().unwrap().push(normal(
                    &json!({"kind": "teardown", "command": command, "cwd": cwd, "timeoutMs": timeout_ms, "environment": environment}),
                    &teardown_scratch,
                ));
                teardown
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(SetupOutcome::Ok)
            })),
            site: Some(FrontEndSite {
                env: HashMap::new(),
                exec_path: "/usr/bin/node".into(),
                sea: false,
                cstan: "/usr/bin/cstan".into(),
            }),
        })
    }

    /// Branches and worktrees as git has them now.
    fn git_state(&self) -> Value {
        let list = git(
            &self.real_git,
            &self.project_root,
            &git_args(&["worktree", "list", "--porcelain"]),
        )
        .expect("git lists the worktrees");
        let mut worktrees: Vec<Vec<String>> = list
            .split("\n\n")
            .map(str::trim)
            .filter(|block| !block.is_empty())
            .map(|block| block.split('\n').map(str::to_string).collect())
            .collect();
        worktrees.sort_by(|a, b| a.first().cmp(&b.first()));
        let refs: Vec<String> = git(
            &self.real_git,
            &self.project_root,
            &git_args(&[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/capstan",
            ]),
        )
        .expect("git lists the refs")
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
        normal(
            &json!({"worktrees": worktrees, "refs": refs}),
            &self.scratch,
        )
    }

    /// The git shim's log from the offset on, one entry per command.
    fn shim_commands(&mut self) -> Value {
        let mut text = String::new();
        std::fs::File::open(&self.git_log)
            .and_then(|mut f| f.read_to_string(&mut text))
            .expect("the shim log reads");
        let new = text[self.git_offset..].to_string();
        self.git_offset = text.len();
        normal(
            &Value::Array(
                new.split('\n')
                    .filter(|line| !line.is_empty())
                    .map(|line| Value::String(line.to_string()))
                    .collect(),
            ),
            &self.scratch,
        )
    }

    fn dump(&self) -> Value {
        self.kernel.call(dump_tables).expect("the kernel dumps")
    }
}

/// Runs one step; the Ok side is the result, the Err side the error as the exporter records it.
fn perform(
    step: &Value,
    launcher: &mut Launcher,
    world: &mut World,
    spec: &Value,
    state_dir: &Path,
) -> Result<Value, Value> {
    let op = step["op"].as_str().unwrap_or("");
    let failure = |message: String| json!({"message": message, "name": "Error"});
    match op {
        "launch_pm" => plain(launcher.launch_pm(), |r| r.to_value()),
        "restart_pm" => plain(launcher.restart_pm(), |r| r.to_value()),
        "spawn" => plain(
            launcher.spawn(&text(step, "role"), &spawn_options(&step["options"])),
            |r| r.to_value(),
        ),
        "release" => plain(launcher.release(&text(step, "agent")), |r| r.to_value()),
        "replace" => plain(launcher.replace(&text(step, "agent")), |r| r.to_value()),
        "adopt_all" => plain(launcher.adopt_all(), |()| Value::Null),
        "observe" => plain(
            launcher.observe(
                &text(step, "agent"),
                step["lines"].as_u64().unwrap_or(40) as usize,
            ),
            |r| r.to_value(),
        ),
        "rename_branch_for_task" => plain(
            launcher.rename_branch_for_task(&text(step, "agent"), &text(step, "task")),
            |r| {
                let mut map = Map::new();
                map.insert("branch".into(), json!(r.branch));
                map.insert("renamed".into(), json!(r.renamed));
                if let Some(note) = r.note {
                    map.insert("note".into(), json!(note));
                }
                Value::Object(map)
            },
        ),
        "interrupt" => plain(launcher.interrupt(&text(step, "agent")), |sent| json!(sent)),
        "status" => Ok(launcher.status().to_value()),
        "operator_environment" => Ok(json!(launcher.operator_environment())),
        "in_flight" => Ok(json!(launcher.in_flight_operations())),
        "reopen" => {
            *launcher = world.make_launcher(spec, state_dir);
            Ok(Value::Null)
        }
        "git" => {
            let cwd = match step["cwd"].as_str() {
                None | Some("root") => world.project_root.clone(),
                Some(other) => PathBuf::from(other),
            };
            let args: Vec<String> = step["args"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap_or("").to_string())
                        .collect()
                })
                .unwrap_or_default();
            git(&world.real_git, &cwd, &args)
                .map(|out| json!({"stdout": out.trim()}))
                .map_err(failure)
        }
        "write" => {
            let file = Path::new(&text(step, "dir")).join(text(step, "file"));
            std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| failure(e.to_string()))?;
            std::fs::write(&file, text(step, "text")).map_err(|e| failure(e.to_string()))?;
            Ok(Value::Null)
        }
        "report" => {
            let agent = text(step, "agent");
            let Some(token) = world.stub.token_of(&agent) else {
                return Err(failure(format!("no token for {agent}")));
            };
            let commit = text(step, "commit");
            let summary = text(step, "summary");
            let recorded = world.kernel.call(move |core| {
                let panes = core.agent_panes(OWNER_CREDENTIAL)?;
                let row = panes
                    .as_array()
                    .and_then(|rows| rows.iter().find(|r| r["agentId"] == agent.as_str()))
                    .cloned()
                    .unwrap_or(Value::Null);
                let ctx = context(core, &token)?;
                core.record_agent_report(
                    &ctx,
                    &json!({
                        "commitSha": commit,
                        "summary": summary,
                        "evidence": {
                            "generation": 1,
                            "branch": row["branch"],
                            "baseSha": row["baseSha"],
                            "commitExists": true,
                            "branchTip": commit,
                            "isAncestorOfTip": true,
                            "isAncestorOfBase": false,
                            "checkedAt": "2026-10-01T00:00:00.000Z",
                        },
                    }),
                )
            });
            match recorded {
                Ok(Ok(value)) => Ok(json!({"state": value["record"]["state"]})),
                Ok(Err(error)) => Err(failure(error.message())),
                Err(_) => Err(failure("the kernel is gone".into())),
            }
        }
        "send" => {
            let from = text(step, "from");
            let Some(token) = world.stub.token_of(&from) else {
                return Err(failure(format!("no token for {from}")));
            };
            let input = json!({"recipientAgentId": text(step, "to"), "body": text(step, "body")});
            match world.kernel.call(move |core| {
                let ctx = context(core, &token)?;
                core.enqueue_message(&ctx, &input)
            }) {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(failure(error.message())),
                Err(_) => Err(failure("the kernel is gone".into())),
            }
        }
        "stub" => {
            match step["do"].as_str() {
                Some("setup") => world.setup_outcomes.lock().unwrap().extend(
                    step["outcomes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(outcome_of),
                ),
                Some("teardown") => world.teardown_outcomes.lock().unwrap().extend(
                    step["outcomes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(outcome_of),
                ),
                _ => world.stub.apply(step),
            }
            Ok(Value::Null)
        }
        other => panic!("unknown op {other}"),
    }
}

/// Replays one sequence; the error names the first difference.
pub fn run_sequence(sequence: &Value) -> Result<(), String> {
    let name = sequence["name"].as_str().unwrap_or("?");
    let seed = sequence["seed"].as_str().unwrap_or(name);
    let spec = sequence["config"].clone();
    let scratch_dir = tempfile::Builder::new()
        .prefix("capstan-launcher-parity-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let scratch = std::fs::canonicalize(scratch_dir.path())
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let scratch_path = PathBuf::from(&scratch);
    let project_root = scratch_path.join("project");
    let state_dir = project_root.join(".capstan").join("state");
    let bin_dir = scratch_path.join("bin");
    let prompts_dir = scratch_path.join("prompts");
    let worktrees_dir = scratch_path.join("worktrees");
    for dir in [&project_root, &bin_dir, &prompts_dir, &worktrees_dir] {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(&state_dir).map_err(|e| e.to_string())?;
    for dir in [project_root.join(".capstan"), state_dir.clone()] {
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let real_git = String::from_utf8_lossy(
        &std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .map_err(|e| e.to_string())?
            .stdout,
    )
    .trim()
    .to_string();
    let git_log = scratch_path.join("git-argv.log");
    std::fs::write(&git_log, "").map_err(|e| e.to_string())?;
    let shim = bin_dir.join("git");
    super::exec::write_script(
        &shim,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{real_git}' \"$@\"\n",
            git_log.display()
        ),
        0o755,
    );

    git(
        &real_git,
        &project_root,
        &git_args(&["init", "--quiet", "-b", "main"]),
    )?;
    std::fs::write(project_root.join("README.md"), "parity\n").map_err(|e| e.to_string())?;
    git(&real_git, &project_root, &git_args(&["add", "-A"]))?;
    git(
        &real_git,
        &project_root,
        &git_args(&["commit", "--quiet", "-m", "chore: initial commit"]),
    )?;

    let saved_path = std::env::var("PATH").ok();
    let saved_front_end = std::env::var("CSTAN_FRONT_END").ok();
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            bin_dir.display(),
            saved_path.as_deref().unwrap_or("/usr/bin:/bin")
        ),
    );
    std::env::remove_var("CSTAN_FRONT_END");
    let outcome = replay(
        sequence,
        seed,
        &spec,
        &scratch,
        &project_root,
        &state_dir,
        &prompts_dir,
        &worktrees_dir,
        &real_git,
        &git_log,
    );
    match saved_path {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }
    if let Some(value) = saved_front_end {
        std::env::set_var("CSTAN_FRONT_END", value);
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
fn replay(
    sequence: &Value,
    seed: &str,
    spec: &Value,
    scratch: &str,
    project_root: &Path,
    state_dir: &Path,
    prompts_dir: &Path,
    worktrees_dir: &Path,
    real_git: &str,
    git_log: &Path,
) -> Result<(), String> {
    let kernel = Arc::new(KernelThread::spawn(state_dir, &project(), seed)?);
    let host_of = |name: &str| -> String {
        spec["hostOf"]
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or("claude")
            .to_string()
    };
    if spec["synced"] != false {
        let desired: Vec<Value> = [
            ("pm", "PM"),
            ("pm2", "PM"),
            ("developer", "Developer"),
            ("developer2", "Developer"),
            ("architect", "Developer"),
            ("operator", "Developer"),
            ("researcher", "Developer"),
            ("supervisor", "Supervisor"),
        ]
        .iter()
        .map(|(name, kind)| {
            json!({"name": name, "kind": kind, "host": host_of(name), "configHash": hash_of(name)})
        })
        .collect();
        kernel
            .call(move |core| {
                let ctx = context(core, OWNER_CREDENTIAL)?;
                core.sync_role_definitions(&ctx, &Value::Array(desired))
            })
            .map_err(|_| "the kernel is gone".to_string())?
            .map_err(|e| e.to_string())?;
    }
    let stub = Arc::new(StubHerdr::new(
        scratch,
        prompts_dir.to_path_buf(),
        &worktrees_dir.to_string_lossy(),
        real_git,
        &project_root.to_string_lossy(),
    ));
    let mut world = World {
        scratch: scratch.to_string(),
        project_root: project_root.to_path_buf(),
        real_git: real_git.to_string(),
        git_log: git_log.to_path_buf(),
        git_offset: 0,
        stub,
        kernel,
        events: Arc::default(),
        sleeps: Arc::default(),
        commands: Arc::default(),
        setup_outcomes: Arc::default(),
        teardown_outcomes: Arc::default(),
        sync_roles: Mutex::new(None),
        clock: Mutex::new(None),
        teardown_advance_ms: Default::default(),
    };
    let mut launcher = world.make_launcher(spec, state_dir);
    let mut bindings: HashMap<String, Value> = HashMap::new();
    let mut previous = world.dump();
    let mut failure: Option<String> = None;
    for (index, recorded) in sequence["steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let mut step = Map::new();
        step.insert("op".into(), recorded["op"].clone());
        if let Some(args) = recorded["args"].as_object() {
            for (key, value) in args {
                step.insert(key.clone(), value.clone());
            }
        }
        let step = match resolve(&Value::Object(step), &bindings) {
            Ok(step) => step,
            Err(message) => {
                failure = Some(format!("step {index}: {message}"));
                break;
            }
        };
        let outcome = perform(&step, &mut launcher, &mut world, spec, state_dir);
        let mut actual = Map::new();
        match &outcome {
            Ok(value) => {
                actual.insert("result".into(), normal(value, scratch));
                if let Some(name) = recorded["as"].as_str() {
                    bindings.insert(name.to_string(), value.clone());
                }
            }
            Err(error) => {
                actual.insert("error".into(), normal(error, scratch));
            }
        }
        actual.insert("git".into(), world.shim_commands());
        actual.insert("calls".into(), Value::Array(world.stub.take_calls()));
        actual.insert(
            "events".into(),
            Value::Array(std::mem::take(&mut *world.events.lock().unwrap())),
        );
        let sleeps = std::mem::take(&mut *world.sleeps.lock().unwrap());
        if !sleeps.is_empty() {
            actual.insert("sleeps".into(), json!(sleeps));
        }
        let commands = std::mem::take(&mut *world.commands.lock().unwrap());
        if !commands.is_empty() {
            actual.insert("commands".into(), Value::Array(commands));
        }
        actual.insert("state".into(), world.git_state());
        let now = world.dump();
        actual.insert(
            "tablesDiff".into(),
            normal(&diff_tables(&previous, &now), scratch),
        );
        previous = now;
        let mut expected = Map::new();
        for key in [
            "result",
            "error",
            "git",
            "calls",
            "events",
            "sleeps",
            "commands",
            "state",
            "tablesDiff",
        ] {
            if let Some(value) = recorded.get(key) {
                expected.insert(key.to_string(), value.clone());
            }
        }
        if let Some(difference) =
            first_difference(&Value::Object(expected), &Value::Object(actual), "step")
        {
            failure = Some(format!(
                "step {index} ({}): {difference}",
                recorded["op"].as_str().unwrap_or("?")
            ));
            break;
        }
    }
    world.kernel.close();
    match failure {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------------ Rust-only scenarios

/// A launcher over the stub Herdr, real git in a scratch repository and a real `Core`, built the way a replayed sequence is,
/// for the tests that state behaviour directly (`tests/rust_only.rs`): the pieces are public so a test can seed the ledger,
/// script the stub and read the log. The scratch `git` shim is first on the process's PATH while it lives.
pub struct Scenario {
    _serial: std::sync::MutexGuard<'static, ()>,
    pub world: World,
    pub launcher: Launcher,
    pub spec: Value,
    pub state_dir: PathBuf,
    pub project_root: PathBuf,
    _scratch: tempfile::TempDir,
    saved_path: Option<String>,
}

impl Scenario {
    /// `spec` is the `config` object of a sequence (`maxWorkers`, `layout`, `operator`, `pass`, ...); roles are synced
    /// unless `spec.synced` is false.
    pub fn new(name: &str, spec: Value) -> Scenario {
        // The scenarios change the process's PATH (the git shim), so one runs at a time.
        static SERIAL: Mutex<()> = Mutex::new(());
        let serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch_dir = tempfile::Builder::new()
            .prefix("capstan-launcher-rust-only-")
            .tempdir()
            .expect("a scratch directory");
        let scratch = std::fs::canonicalize(scratch_dir.path())
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let scratch_path = PathBuf::from(&scratch);
        let project_root = scratch_path.join("project");
        let state_dir = project_root.join(".capstan").join("state");
        let (bin_dir, prompts_dir, worktrees_dir) = (
            scratch_path.join("bin"),
            scratch_path.join("prompts"),
            scratch_path.join("worktrees"),
        );
        for dir in [
            &project_root,
            &bin_dir,
            &prompts_dir,
            &worktrees_dir,
            &state_dir,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for dir in [project_root.join(".capstan"), state_dir.clone()] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let real_git = String::from_utf8_lossy(
            &std::process::Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        let git_log = scratch_path.join("git-argv.log");
        std::fs::write(&git_log, "").unwrap();
        super::exec::write_script(
            &bin_dir.join("git"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{real_git}' \"$@\"\n",
                git_log.display()
            ),
            0o755,
        );
        git(
            &real_git,
            &project_root,
            &git_args(&["init", "--quiet", "-b", "main"]),
        )
        .unwrap();
        std::fs::write(project_root.join("README.md"), "rust only\n").unwrap();
        git(&real_git, &project_root, &git_args(&["add", "-A"])).unwrap();
        git(
            &real_git,
            &project_root,
            &git_args(&["commit", "--quiet", "-m", "chore: initial commit"]),
        )
        .unwrap();
        let saved_path = std::env::var("PATH").ok();
        std::env::set_var(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                saved_path.as_deref().unwrap_or("/usr/bin:/bin")
            ),
        );
        std::env::remove_var("CSTAN_FRONT_END");

        let kernel =
            Arc::new(KernelThread::spawn(&state_dir, &project(), name).expect("the ledger opens"));
        let host_of = |role: &str| {
            spec["hostOf"]
                .get(role)
                .and_then(Value::as_str)
                .unwrap_or("claude")
                .to_string()
        };
        if spec["synced"] != false {
            let desired: Vec<Value> = ROLES
                .iter()
                .map(|(role, kind)| json!({"name": role, "kind": kind, "host": host_of(role), "configHash": hash_of(role)}))
                .collect();
            kernel
                .call(move |core| {
                    let ctx = context(core, OWNER_CREDENTIAL)?;
                    core.sync_role_definitions(&ctx, &Value::Array(desired))
                })
                .expect("the kernel runs")
                .expect("the roles sync");
        }
        let stub = Arc::new(StubHerdr::new(
            &scratch,
            prompts_dir,
            &worktrees_dir.to_string_lossy(),
            &real_git,
            &project_root.to_string_lossy(),
        ));
        let world = World {
            scratch,
            project_root: project_root.clone(),
            real_git,
            git_log,
            git_offset: 0,
            stub,
            kernel,
            events: Arc::default(),
            sleeps: Arc::default(),
            commands: Arc::default(),
            setup_outcomes: Arc::default(),
            teardown_outcomes: Arc::default(),
            sync_roles: Mutex::new(None),
            clock: Mutex::new(None),
            teardown_advance_ms: Default::default(),
        };
        let launcher = world.make_launcher(&spec, &state_dir);
        Scenario {
            _serial: serial,
            world,
            launcher,
            spec,
            state_dir,
            project_root,
            _scratch: scratch_dir,
            saved_path,
        }
    }

    /// A new launcher over the same ledger and stub (a daemon restart).
    pub fn reopen(&mut self) {
        self.launcher = self.world.make_launcher(&self.spec, &self.state_dir);
    }

    /// Runs `f` on the ledger's controller thread.
    pub fn ledger<R: Send + 'static>(
        &self,
        f: impl FnOnce(&capstan_kernel::Core) -> capstan_kernel::KernelResult<R> + Send + 'static,
    ) -> R {
        self.world
            .kernel
            .call(f)
            .expect("the kernel runs")
            .unwrap_or_else(|e| panic!("the ledger refused: {e}"))
    }

    /// The owner's context on `core`.
    pub fn owner(core: &capstan_kernel::Core) -> capstan_kernel::types::MutationContext {
        context(core, OWNER_CREDENTIAL).expect("a context")
    }

    /// The log events recorded since the last call, as `[{event, details}]`.
    pub fn take_events(&self) -> Vec<Value> {
        std::mem::take(&mut *self.world.events.lock().unwrap())
    }

    /// The prompt files the stub wrote, in the order the agents were started.
    pub fn prompts(&self) -> Vec<String> {
        let directory = Path::new(&self.world.scratch).join("prompts");
        let mut files: Vec<(usize, PathBuf)> = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                let number = path
                    .file_stem()?
                    .to_str()?
                    .strip_prefix("prompt-")?
                    .parse()
                    .ok()?;
                Some((number, path))
            })
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|(_, path)| std::fs::read_to_string(path).unwrap())
            .collect()
    }

    /// The calls to the stub Herdr since the last call.
    pub fn take_calls(&self) -> Vec<Value> {
        self.world.stub.take_calls()
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        self.world.kernel.close();
        match self.saved_path.take() {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

/// The roles every scenario's ledger knows: name and kind.
pub const ROLES: [(&str, &str); 8] = [
    ("pm", "PM"),
    ("pm2", "PM"),
    ("developer", "Developer"),
    ("developer2", "Developer"),
    ("architect", "Developer"),
    ("operator", "Developer"),
    ("researcher", "Developer"),
    ("supervisor", "Supervisor"),
];

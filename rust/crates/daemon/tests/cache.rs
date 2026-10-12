//! The `cache` route (prune and clear), the `cache` member of `status` and the cache loop, through
//! `handlers::handle_frame` on a scratch git project, a real ledger and a cache root of sparse files.

mod common;

use capstan_cache::{read_manifest, Runtime};
use capstan_daemon::deps::Loops;
use capstan_daemon::deps::{DaemonOptions, Deps, KernelHandle, LogEntry, Logger};
use capstan_daemon::handlers::cache::{refresh_manifest, runtime_of_deps};
use capstan_daemon::handlers::handle_frame;
use capstan_daemon::handlers::shared::AbortSignal;
use capstan_daemon::handlers::MAX_RESPONSE_BYTES;
use capstan_daemon::loops::cache::{cycle, run, Timing};
use capstan_daemon::loops::driver::DriverLog;
use capstan_daemon::loops::{RustLoops, StopFlag};
use capstan_kernel::kernel::KernelOptions;
use capstan_kernel::Core;
use common::project::{build_repo, write_project, Identity};
use common::{private_tempdir, seeded_env};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const BASE_CONFIG: &str = r#"schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
"#;

const MB: u64 = 1 << 20;

struct World {
    dir: tempfile::TempDir,
    cache_dir: tempfile::TempDir,
    deps: Deps,
    credentials: HashMap<String, String>,
    logs: Arc<Mutex<Vec<LogEntry>>>,
}

fn cache_section(root: &Path, extra: &str) -> String {
    format!(
        "[cache]\nroot = \"{}\"\npresets = [\"python\"]\nmax_size_gb = 0.1\nmax_age_days = 30\n{extra}\n\
         [cache.entries.bundler]\ndirs = {{ BUNDLE_PATH = \"bundle\" }}\n\
         [cache.entries.ghost]\ndirs = {{ GHOST_HOME = \"g\" }}\nrequires = \"ghost-tool-that-is-not-installed\"\n",
        root.display()
    )
}

impl World {
    /// `cache` is the text after `[cache]`'s own keys; `None` leaves the section out.
    fn new(with_cache: bool, extra: &str) -> World {
        let dir = private_tempdir();
        let cache_dir = private_tempdir();
        let identity = Identity {
            project_id: "pcache".into(),
            name: "Cache".into(),
            credential: format!("operator-{}", "d".repeat(40)),
        };
        let state = write_project(dir.path(), &identity);
        let mut config = BASE_CONFIG.to_string();
        if with_cache {
            config.push_str(&cache_section(cache_dir.path(), extra));
        }
        let file = dir.path().join("capstan.toml");
        std::fs::write(&file, config).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        build_repo(
            dir.path(),
            &json!({"files": {"a.txt": "a\n"}, "branches": {}}),
        );
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
                    seeded_env("cache"),
                )
            })
            .expect("the kernel opens")
        };
        let logs: Arc<Mutex<Vec<LogEntry>>> = Arc::default();
        let sink: Logger = {
            let logs = logs.clone();
            Arc::new(move |entry: &LogEntry| logs.lock().unwrap().push(entry.clone()))
        };
        let mut options = DaemonOptions::new(state, project, dir.path().to_path_buf());
        options.capstan = Some(Arc::new(
            capstan_config::load_role_config(dir.path()).unwrap(),
        ));
        let deps = Deps::new(kernel, options, sink);
        capstan_daemon::deps::startup_sequence(&deps);
        let mut world = World {
            dir,
            cache_dir,
            deps,
            credentials: HashMap::new(),
            logs,
        };
        world
            .credentials
            .insert("operator".into(), identity.credential);
        world.populate();
        world
    }

    fn ledger(&self, credential: &str, op: &str, args: Vec<Value>) -> Value {
        let (credential, name) = (credential.to_string(), op.to_string());
        self.deps
            .kernel
            .run(move |core| {
                let context = capstan_daemon::deps::new_context(core, &credential)?;
                let mut all = vec![serde_json::to_value(context).unwrap()];
                all.extend(args);
                core.dispatch(&name, &all)
            })
            .unwrap_or_else(|e| panic!("{op}: {e}"))
    }

    fn populate(&mut self) {
        let owner = self.credentials["operator"].clone();
        let roles = json!([
            {"name": "pm", "kind": "PM", "host": "claude", "configHash": "a".repeat(64)},
            {"name": "developer", "kind": "Developer", "host": "claude", "configHash": "a".repeat(64)},
        ]);
        self.ledger(&owner, "syncRoleDefinitions", vec![roles]);
        for (agent, role, kind) in [("pm-1", "pm", "PM"), ("dev-1", "developer", "Developer")] {
            let seat = format!("seat-{agent}");
            self.ledger(
                &owner,
                "createSeat",
                vec![json!({"seatId": seat, "name": seat, "role": kind})],
            );
            let actor = self.ledger(
                &owner,
                "createActor",
                vec![json!({"displayName": agent, "role": kind, "seatId": seat})],
            );
            self.ledger(
                &owner,
                "registerAgent",
                vec![json!({"agentId": agent, "roleName": role, "seatId": seat, "actorId": actor["actorId"]})],
            );
            self.credentials
                .insert(agent.into(), actor["credential"].as_str().unwrap().into());
        }
    }

    fn call(&self, who: &str, command: &str, args: &[&str]) -> Value {
        let credential = self.credentials[who].clone();
        let frame = json!({"v": 1, "credential": credential, "command": command, "args": args});
        let outcome = handle_frame(
            &self.deps,
            frame.to_string().as_bytes(),
            &AbortSignal::new(),
            MAX_RESPONSE_BYTES,
        );
        serde_json::from_str(&outcome.response.expect("an answer")).expect("JSON")
    }

    fn ok(&self, who: &str, args: &[&str]) -> Value {
        let answer = self.call(who, "cache", args);
        assert_eq!(answer["ok"], true, "cache {args:?}: {answer}");
        answer["result"].clone()
    }

    fn refused(&self, who: &str, args: &[&str], code: &str) -> String {
        let answer = self.call(who, "cache", args);
        assert_eq!(answer["ok"], false, "cache {args:?}: {answer}");
        assert_eq!(answer["code"], code, "cache {args:?}: {answer}");
        answer["message"].as_str().unwrap().to_string()
    }

    fn root(&self) -> PathBuf {
        self.runtime().root
    }

    fn runtime(&self) -> Runtime {
        runtime_of_deps(&self.deps).expect("a [cache] section")
    }

    /// An entry of `bytes` apparent bytes last used `age_days` ago.
    fn fill(&self, entry: &str, bytes: u64, age_days: u64) {
        fill(&self.root().join(entry), bytes, age_days);
    }

    fn exists(&self, entry: &str) -> bool {
        self.root().join(entry).is_dir()
    }

    fn git(&self, args: &[&str]) {
        git_in(self.dir.path(), args);
    }

    fn status_cache(&self) -> Value {
        let answer = self.call("operator", "status", &[]);
        assert_eq!(answer["ok"], true, "{answer}");
        answer["result"]["cache"].clone()
    }
}

fn fill(dir: &Path, bytes: u64, age_days: u64) {
    std::fs::create_dir_all(dir).unwrap();
    File::create(dir.join("blob"))
        .unwrap()
        .set_len(bytes)
        .unwrap();
    let stamp = dir.join(".last-used");
    let file = File::create(&stamp).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(age_days * 86_400))
        .unwrap();
}

fn git_in(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_worker_is_refused_and_the_pm_and_the_operator_run() {
    let world = World::new(true, "");
    world.fill("python", 10 * MB, 1);
    world.refused("dev-1", &["prune", "--dry-run"], "forbidden");
    world.refused("dev-1", &["clear", "python"], "forbidden");
    assert!(world.exists("python"));
    let dry = world.ok("pm-1", &["prune", "--dry-run"]);
    assert_eq!(dry["dryRun"], true);
    let cleared = world.ok("operator", &["clear", "python"]);
    assert_eq!(cleared["cleared"], json!(["python"]));
    assert!(!world.exists("python"));
}

#[test]
fn the_route_needs_a_cache_section_and_valid_arguments() {
    let none = World::new(false, "");
    none.refused("pm-1", &["prune"], "invalid_request");
    let world = World::new(true, "");
    world.refused("pm-1", &[], "invalid_request");
    world.refused("pm-1", &["clear"], "invalid_request");
    world.refused("pm-1", &["clear", "nope"], "invalid_request");
    world.refused("pm-1", &["clear", "--all", "python"], "invalid_request");
    world.refused("pm-1", &["prune", "--bad"], "invalid_request");
    world.refused("pm-1", &["prune", "nope"], "invalid_request");
}

#[test]
fn clear_removes_only_the_named_entry_and_the_manifest_is_measured_again() {
    let world = World::new(true, "");
    world.fill("python", 5 * MB, 1);
    world.fill("bundler", 7 * MB, 1);
    refresh_manifest(&world.runtime(), None).unwrap();
    let result = world.ok("pm-1", &["clear", "python"]);
    assert_eq!(result["cleared"], json!(["python"]));
    assert!(!world.exists("python") && world.exists("bundler"));
    assert_eq!(result["totalBytes"], 7 * MB);
    let manifest = read_manifest(&world.root()).unwrap().unwrap();
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].name, "bundler");
}

#[test]
fn clear_all_needs_force_while_a_worker_has_a_worktree() {
    let world = World::new(true, "");
    world.fill("python", MB, 1);
    world.fill("bundler", MB, 1);
    let worktree = world.cache_dir.path().join("wt");
    world.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "feat/dev-1",
        worktree.to_str().unwrap(),
    ]);
    let message = world.refused("pm-1", &["clear", "--all"], "rejected");
    assert!(message.contains("--force"), "{message}");
    assert!(world.exists("python") && world.exists("bundler"));
    let result = world.ok("pm-1", &["clear", "--all", "--force"]);
    assert_eq!(result["cleared"].as_array().unwrap().len(), 2, "{result}");
    assert!(!world.exists("python") && !world.exists("bundler"));
}

#[test]
fn clearing_a_cache_of_scope_user_needs_force() {
    let world = World::new(true, "scope = \"user\"");
    world.fill("python", MB, 1);
    let message = world.refused("pm-1", &["clear", "python"], "rejected");
    assert!(message.contains("scope user"), "{message}");
    assert!(world.exists("python"));
    world.ok("pm-1", &["clear", "python", "--force"]);
    assert!(!world.exists("python"));
}

#[test]
fn a_dry_run_lists_what_would_be_pruned_and_changes_nothing() {
    let world = World::new(true, "");
    world.fill("python", 80 * MB, 40);
    world.fill("bundler", 80 * MB, 1);
    let before = refresh_manifest(&world.runtime(), Some(5)).unwrap();
    let result = world.ok("pm-1", &["prune", "--dry-run"]);
    assert_eq!(result["dryRun"], true);
    assert_eq!(result["cleared"], json!([]));
    let would: Vec<&str> = result["wouldClear"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(would.contains(&"python"), "{result}");
    assert!(world.exists("python") && world.exists("bundler"));
    let after = read_manifest(&world.root()).unwrap().unwrap();
    assert_eq!(after.entries, before.entries);
    assert_eq!(after.last_prune_at_ms, Some(5));
}

#[test]
fn prune_over_the_cap_clears_the_least_recently_used_until_under_it() {
    let world = World::new(true, "");
    world.fill("python", 60 * MB, 5);
    world.fill("bundler", 60 * MB, 1);
    let result = world.ok("pm-1", &["prune"]);
    assert_eq!(result["idle"], true);
    assert_eq!(result["cleared"], json!(["python"]), "{result}");
    assert!(!world.exists("python") && world.exists("bundler"));
    assert_eq!(result["totalAfter"], 60 * MB);
    let manifest = read_manifest(&world.root()).unwrap().unwrap();
    assert!(manifest.last_prune_at_ms.is_some());
    assert_eq!(manifest.entries.len(), 1);
}

#[test]
fn prune_does_not_clear_while_a_worktree_exists() {
    let world = World::new(true, "");
    world.fill("python", 60 * MB, 5);
    world.fill("bundler", 60 * MB, 1);
    let worktree = world.cache_dir.path().join("wt");
    world.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "feat/dev-1",
        worktree.to_str().unwrap(),
    ]);
    let result = world.ok("pm-1", &["prune"]);
    assert_eq!(result["idle"], false);
    assert_eq!(result["cleared"], json!([]));
    assert_eq!(result["wouldClear"], json!(["python"]));
    assert!(world.exists("python"));
}

#[test]
fn prune_can_be_limited_to_named_entries_and_keeps_the_others_in_the_manifest() {
    let world = World::new(true, "");
    world.fill("python", MB, 1);
    world.fill("bundler", MB, 1);
    world.ok("pm-1", &["prune", "python"]);
    let manifest = read_manifest(&world.root()).unwrap().unwrap();
    let names: Vec<&str> = manifest.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["python", "bundler"]);
}

#[test]
fn the_setup_entry_is_listed_and_clearing_one_snapshot_keeps_the_others_in_the_manifest() {
    let world = World::new(
        true,
        "\n[cache.setup_reuse]\nkey_files = [\"a.txt\"]\npaths = [\"target\"]\n",
    );
    fill(&world.root().join("setup").join("k1"), 3 * MB, 1);
    fill(&world.root().join("setup").join("k2"), 4 * MB, 1);
    refresh_manifest(&world.runtime(), None).unwrap();
    let member = world.status_cache();
    let setup = member["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "setup")
        .expect("the setup entry");
    assert_eq!(setup["bytes"], 7 * MB);
    let result = world.ok("pm-1", &["clear", "setup/k1"]);
    assert_eq!(result["cleared"], json!(["setup/k1"]));
    assert!(world.root().join("setup").join("k2").is_dir());
    assert!(!world.root().join("setup").join("k1").exists());
    let member = world.status_cache();
    let setup = member["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "setup")
        .unwrap();
    assert_eq!(setup["bytes"], 4 * MB, "{member}");
}

#[test]
fn status_has_a_cache_member_from_the_manifest_and_none_without_the_section() {
    let world = World::new(true, "");
    // Before any measurement: the entries are there, the sizes are not.
    let member = world.status_cache();
    assert_eq!(member["totalBytes"], Value::Null);
    assert_eq!(member["measuredAt"], Value::Null);
    let ghost = member["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "ghost")
        .unwrap();
    assert_eq!(ghost["enabled"], false);
    assert!(ghost["reason"].as_str().unwrap().contains("ghost-tool"));
    world.fill("python", 2 * MB, 1);
    refresh_manifest(&world.runtime(), None).unwrap();
    let member = world.status_cache();
    assert_eq!(member["totalBytes"], 2 * MB);
    assert_eq!(
        member["maxBytes"],
        (0.1 * (1u64 << 30) as f64).round() as u64
    );
    assert_eq!(member["scope"], "project");
    assert!(member["root"].as_str().unwrap().len() > 1);
    assert!(member["measuredAt"].is_string());
    assert!(member["warnings"].is_array());
    let python = member["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "python")
        .unwrap();
    assert_eq!(python["bytes"], 2 * MB);
    assert!(python["lastUsedAt"].is_string());

    let plain = World::new(false, "");
    let answer = plain.call("operator", "status", &[]);
    assert!(answer["result"].get("cache").is_none());
}

#[test]
fn a_restarted_daemon_serves_the_manifest_and_removes_a_leftover_trash() {
    let world = World::new(true, "");
    world.fill("python", 2 * MB, 1);
    refresh_manifest(&world.runtime(), None).unwrap();
    let trash = world.root().join(".trash").join("python-1");
    std::fs::create_dir_all(&trash).unwrap();
    std::fs::write(trash.join("old"), "x").unwrap();
    let loops = RustLoops::new();
    loops.start_ticks(&world.deps);
    // The status is served from the manifest at once; the first new measurement is a while away.
    assert_eq!(world.status_cache()["totalBytes"], 2 * MB);
    for _ in 0..200 {
        if !world.root().join(".trash").join("python-1").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    loops.stop_ticks();
    assert!(!trash.exists(), "the leftover trash is gone");
    assert_eq!(world.status_cache()["totalBytes"], 2 * MB);
}

#[test]
fn the_loop_prunes_and_writes_the_manifest_and_stops_at_once() {
    let world = World::new(true, "");
    world.fill("python", 60 * MB, 5);
    world.fill("bundler", 60 * MB, 1);
    let log: DriverLog = Arc::new(|_, _| {});
    let runtime = world.runtime();
    let stop = StopFlag::new();
    let handle = {
        let (runtime, stop, log) = (runtime.clone(), stop.clone(), log.clone());
        std::thread::spawn(move || {
            run(
                &runtime,
                &stop,
                Timing {
                    first_delay: Duration::from_millis(10),
                    interval: Duration::from_secs(3600),
                },
                &log,
            )
        })
    };
    for _ in 0..200 {
        if read_manifest(&runtime.root)
            .unwrap()
            .is_some_and(|m| m.last_prune_at_ms.is_some())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.stop();
    handle.join().unwrap();
    assert!(!world.exists("python") && world.exists("bundler"));
    let manifest = read_manifest(&runtime.root).unwrap().unwrap();
    assert!(manifest.last_prune_at_ms.is_some());
    assert!(world.logs.lock().unwrap().iter().all(|_| true));
}

#[test]
fn one_cycle_that_finds_a_worktree_only_reports() {
    let world = World::new(true, "");
    world.fill("python", 60 * MB, 5);
    world.fill("bundler", 60 * MB, 1);
    let worktree = world.cache_dir.path().join("wt");
    world.git(&[
        "worktree",
        "add",
        "--quiet",
        "-b",
        "feat/dev-1",
        worktree.to_str().unwrap(),
    ]);
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let log: DriverLog = {
        let seen = seen.clone();
        Arc::new(move |event, details| seen.lock().unwrap().push((event.to_string(), details)))
    };
    cycle(&world.runtime(), &log).unwrap();
    assert!(world.exists("python"));
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0, "cache_pruned");
    assert_eq!(seen[0].1["idle"], false);
    assert_eq!(seen[0].1["wouldClear"], json!(["python"]));
}

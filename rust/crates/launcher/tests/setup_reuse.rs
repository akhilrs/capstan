//! `[cache.setup_reuse]`: a setup result is restored by lockfile hash instead of running the setup again.
mod common;

use capstan_herdr::api::StubAdapter;
use capstan_launcher::api::SetupOutcome;
use capstan_launcher::kernel::Ledger;
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{Budget, FrontEndSite, LauncherOptions, LogFn, SetupRunner};
use common::ledger::KernelThread;
use common::test_config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const OWNER: &str = "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";

fn project_json() -> Value {
    json!({
        "projectId": "proj1",
        "name": "Cache",
        "ownerCredential": OWNER,
        "initialInputs": [
            {"kind": "project_config", "content": {"name": "cache"}},
            {"kind": "task_brief", "content": {"objective": "prove it"}},
            {"kind": "acceptance_criteria", "content": {"criteria": ["it works"]}},
            {"kind": "policy", "content": {"review": "required"}},
            {"kind": "plan", "content": {"steps": ["one"]}},
        ],
    })
}

fn kernel_for(project: &Path) -> Arc<KernelThread> {
    let capstan = project.join(".capstan");
    let state = capstan.join("state");
    for dir in [&capstan, &state] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    Arc::new(KernelThread::spawn(&state, &project_json(), "cache").unwrap())
}

type Events = Arc<Mutex<Vec<(String, Value)>>>;

struct Fixture {
    launcher: Launcher,
    events: Events,
    runs: Arc<AtomicUsize>,
    worktree: capstan_config::Worktree,
}

/// `fail`: the stub setup writes node_modules and then exits 1.
fn fixture(project: &Path, toml: Option<&str>, setup_body: &'static str, fail: bool) -> Fixture {
    let mut config = test_config(&json!({"worktree": {"setup": "npm ci"}}));
    if let Some(toml) = toml {
        config.cache = capstan_config::parse_config(toml.as_bytes(), project)
            .unwrap()
            .cache;
    }
    let worktree = config.worktree.clone().unwrap();
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let log: LogFn = Arc::new(move |event, details| {
        sink.lock().unwrap().push((event.to_string(), details));
    });
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    let runner: SetupRunner = Arc::new(move |_, cwd, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        if Path::new(cwd).join(".fail").exists() {
            return SetupOutcome::Failed {
                exit_code: Some(1),
                output: String::new(),
            };
        }
        let dir = Path::new(cwd).join("node_modules");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dep.js"), "x").unwrap();
        match setup_body {
            "venv" => std::fs::write(dir.join("pyvenv.cfg"), "venv").unwrap(),
            "abs-link" => std::os::unix::fs::symlink(dir.join("dep.js"), dir.join("l")).unwrap(),
            "rel-link" => std::os::unix::fs::symlink("dep.js", dir.join("l")).unwrap(),
            _ => {}
        }
        if fail {
            SetupOutcome::Failed {
                exit_code: Some(1),
                output: String::new(),
            }
        } else {
            SetupOutcome::Ok
        }
    });
    let launcher = Launcher::new(LauncherOptions {
        ledger: Ledger::new(kernel_for(project), OWNER),
        adapter: Arc::new(StubAdapter::new()),
        config,
        project_root: project.to_string_lossy().into_owned(),
        project_id: "proj1".into(),
        cli_path: "/opt/capstan/cli.js".into(),
        socket_path: project.join("control.sock").to_string_lossy().into_owned(),
        credential: OWNER.into(),
        node_path: Some("/usr/bin/node".into()),
        base_environment: HashMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]),
        git: None,
        now: None,
        log: Some(log),
        sync_roles: None,
        run_setup: Some(runner),
        sleep: None,
        run_teardown: None,
        site: Some(FrontEndSite {
            env: HashMap::new(),
            exec_path: "/usr/bin/node".into(),
            sea: false,
            cstan: "/usr/bin/cstan".into(),
        }),
    });
    Fixture {
        launcher,
        events,
        runs,
        worktree,
    }
}

fn count(events: &Events, name: &str) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|(e, _)| e == name)
        .count()
}

fn toml(root: &Path, reuse: bool) -> String {
    toml_paths(root, reuse.then_some("\"node_modules\""))
}

fn toml_paths(root: &Path, paths: Option<&str>) -> String {
    let reuse = paths.map_or(String::new(), |paths| {
        format!("[cache.setup_reuse]\nkey_files = [\"package-lock.json\"]\npaths = [{paths}]\n")
    });
    format!("schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[cache]\nroot = \"{}\"\npresets = [\"node\"]\n{reuse}", root.display())
}

/// A plain directory: not a git repository.
fn plain_tree(lock: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("package-lock.json"), lock).unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// A worktree of a real git repository.
fn tree(lock: &str) -> tempfile::TempDir {
    let dir = plain_tree(lock);
    git(dir.path(), &["init", "-q"]);
    dir
}

fn setup(f: &Fixture, wt: &Path) -> Result<(), String> {
    let budget = Budget::new(Arc::new(|| 0), 60_000);
    f.launcher
        .kernel()
        .setup_worktree(&f.worktree, "developer-1", &wt.to_string_lossy(), &budget)
        .map_err(|e| e.message())
}

fn keys(root: &Path) -> Vec<String> {
    let dir = root.join("setup");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn cache_root(f: &Fixture) -> std::path::PathBuf {
    f.launcher.kernel().cache_runtime().unwrap().root.clone()
}

#[test]
fn the_second_spawn_restores_and_a_new_lockfile_runs_setup_again() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "",
        false,
    );
    let first = tree("v1");
    setup(&f, first.path()).unwrap();
    assert_eq!(f.runs.load(Ordering::SeqCst), 1);
    assert_eq!(keys(&cache_root(&f)).len(), 1);

    let second = tree("v1");
    setup(&f, second.path()).unwrap();
    assert_eq!(f.runs.load(Ordering::SeqCst), 1, "setup must not run again");
    assert_eq!(
        std::fs::read_to_string(second.path().join("node_modules/dep.js")).unwrap(),
        "x"
    );
    assert_eq!(count(&f.events, "setup_reused"), 1);

    let third = tree("v2");
    setup(&f, third.path()).unwrap();
    assert_eq!(f.runs.load(Ordering::SeqCst), 2);
    assert_eq!(keys(&cache_root(&f)).len(), 2);
}

#[test]
fn a_failing_setup_leaves_no_snapshot() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "",
        true,
    );
    let wt = tree("v1");
    assert!(setup(&f, wt.path()).is_err());
    assert!(keys(&cache_root(&f)).is_empty());
}

#[test]
fn concurrent_spawns_with_one_key_keep_one_snapshot() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = Arc::new(fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "",
        false,
    ));
    let trees: Vec<_> = (0..6).map(|_| tree("same")).collect();
    let handles: Vec<_> = trees
        .iter()
        .map(|t| {
            let f = Arc::clone(&f);
            let path = t.path().to_path_buf();
            std::thread::spawn(move || setup(&f, &path))
        })
        .collect();
    for h in handles {
        h.join().unwrap().unwrap();
    }
    let names = keys(&cache_root(&f));
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(!names[0].contains(".tmp-"));
}

#[test]
fn a_venv_is_refused_with_a_warning_and_setup_runs() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "venv",
        false,
    );
    for _ in 0..2 {
        setup(&f, tree("v1").path()).unwrap();
    }
    assert_eq!(f.runs.load(Ordering::SeqCst), 2);
    assert!(keys(&cache_root(&f)).is_empty());
    assert_eq!(count(&f.events, "setup_reuse_refused"), 2);
}

#[test]
fn a_missing_key_file_or_no_section_runs_setup_unchanged() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "",
        false,
    );
    let bare = tempfile::tempdir().unwrap();
    setup(&f, bare.path()).unwrap();
    setup(&f, bare.path()).unwrap();
    assert_eq!(f.runs.load(Ordering::SeqCst), 2);

    let project2 = tempfile::tempdir().unwrap();
    let project3 = tempfile::tempdir().unwrap();
    let off = fixture(
        project2.path(),
        Some(&toml(&outside.path().join("d"), false)),
        "",
        false,
    );
    setup(&off, tree("v1").path()).unwrap();
    setup(&off, tree("v1").path()).unwrap();
    assert_eq!(off.runs.load(Ordering::SeqCst), 2);
    let none = fixture(project3.path(), None, "", false);
    setup(&none, tree("v1").path()).unwrap();
    assert_eq!(none.runs.load(Ordering::SeqCst), 1);
    assert_eq!(count(&none.events, "setup_snapshot_stored"), 0);
}

fn refused_twice(f: &Fixture, make: impl Fn() -> tempfile::TempDir) {
    for _ in 0..2 {
        setup(f, make().path()).unwrap();
    }
    assert_eq!(f.runs.load(Ordering::SeqCst), 2);
    assert!(keys(&cache_root(f)).is_empty());
    assert_eq!(count(&f.events, "setup_reuse_refused"), 2);
}

#[test]
fn a_path_tracked_by_git_is_refused() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "",
        false,
    );
    refused_twice(&f, || {
        let t = tree("v1");
        std::fs::create_dir(t.path().join("node_modules")).unwrap();
        std::fs::write(t.path().join("node_modules/a.js"), "a").unwrap();
        git(t.path(), &["add", "-f", "node_modules/a.js"]);
        t
    });
}

#[test]
fn a_git_failure_refuses_instead_of_assuming_untracked() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "",
        false,
    );
    refused_twice(&f, || plain_tree("v1"));
}

#[test]
fn an_absolute_symlink_is_refused_and_a_relative_one_is_kept() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "abs-link",
        false,
    );
    refused_twice(&f, || tree("v1"));

    let project = tempfile::tempdir().unwrap();
    let g = fixture(
        project.path(),
        Some(&toml(&outside.path().join("d"), true)),
        "rel-link",
        false,
    );
    setup(&g, tree("v1").path()).unwrap();
    let second = tree("v1");
    setup(&g, second.path()).unwrap();
    assert_eq!(g.runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_link(second.path().join("node_modules/l")).unwrap(),
        Path::new("dep.js")
    );
}

#[test]
fn a_partial_snapshot_falls_through_to_setup_and_is_replaced() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let toml = toml_paths(
        &outside.path().join("c"),
        Some("\"node_modules\", \"dist\""),
    );
    let f = fixture(project.path(), Some(&toml), "", false);
    // The stub never creates dist, so the stored snapshot lacks it.
    setup(&f, tree("v1").path()).unwrap();
    setup(&f, tree("v1").path()).unwrap();
    assert_eq!(f.runs.load(Ordering::SeqCst), 2);
    assert_eq!(count(&f.events, "setup_reused"), 0);
}

#[test]
fn a_restore_error_removes_only_what_it_created() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let f = fixture(
        project.path(),
        Some(&toml(&outside.path().join("c"), true)),
        "",
        false,
    );
    setup(&f, tree("v1").path()).unwrap();
    let key = keys(&cache_root(&f)).remove(0);
    let blocked = cache_root(&f)
        .join("setup")
        .join(key)
        .join("node_modules/dep.js");
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&blocked).is_ok() {
        return; // running as root: permissions do not apply
    }
    // The second setup fails, so nothing but the restore could leave node_modules behind.
    let wt = tree("v1");
    std::fs::write(wt.path().join(".fail"), "").unwrap();
    assert!(setup(&f, wt.path()).is_err());
    assert_eq!(count(&f.events, "setup_restore_failed"), 1);
    assert!(!wt.path().join("node_modules").exists());
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o600)).unwrap();
}

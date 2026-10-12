//! The `[cache]` environment reaches agents, the setup hook and the teardown hook, and never blocks a spawn.
mod common;

use capstan_herdr::api::StubAdapter;
use capstan_launcher::api::SetupOutcome;
use capstan_launcher::kernel::Ledger;
use capstan_launcher::launcher::Launcher;
use capstan_launcher::shared::{Budget, FrontEndSite, LauncherOptions, LogFn, TeardownRunner};
use common::ledger::KernelThread;
use common::test_config;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
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
}

fn fixture(
    project: &Path,
    toml: Option<&str>,
    path: &str,
    teardown: Option<TeardownRunner>,
) -> Fixture {
    let mut config = test_config(&json!({"worktree": {"teardown": "true"}}));
    if let Some(toml) = toml {
        config.cache = capstan_config::parse_config(toml.as_bytes(), project)
            .unwrap()
            .cache;
    }
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    let log: LogFn = Arc::new(move |event, details| {
        sink.lock().unwrap().push((event.to_string(), details));
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
        base_environment: HashMap::from([("PATH".to_string(), path.to_string())]),
        git: None,
        now: None,
        log: Some(log),
        sync_roles: None,
        run_setup: None,
        sleep: None,
        run_teardown: teardown,
        site: Some(FrontEndSite {
            env: HashMap::new(),
            exec_path: "/usr/bin/node".into(),
            sea: false,
            cstan: "/usr/bin/cstan".into(),
        }),
    });
    Fixture { launcher, events }
}

fn count(events: &Events, name: &str) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|(e, _)| e == name)
        .count()
}

fn cache_toml(root: &Path, body: &str) -> String {
    format!("schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[cache]\nroot = \"{}\"\n{body}", root.display())
}

#[test]
fn agents_setup_and_teardown_share_the_cache_environment() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cache = outside.path().join("root");
    let toml = cache_toml(&cache, "presets = [\"python\", \"node\"]\n");
    let seen: Arc<Mutex<Option<BTreeMap<String, String>>>> = Arc::new(Mutex::new(None));
    let runner: TeardownRunner = {
        let seen = Arc::clone(&seen);
        Arc::new(move |_, _, _, environment| {
            *seen.lock().unwrap() = Some(environment.clone());
            SetupOutcome::Ok
        })
    };
    let f = fixture(project.path(), Some(&toml), "/usr/bin:/bin", Some(runner));
    let kernel = f.launcher.kernel();
    let agent = kernel.environment(Some("token"), None).unwrap();
    let root = kernel
        .cache_runtime()
        .unwrap()
        .root
        .to_string_lossy()
        .into_owned();
    assert!(root.starts_with(&cache.to_string_lossy().into_owned()));
    assert_eq!(agent["CAPSTAN_CACHE_DIR"], root);
    for name in ["PIP_CACHE_DIR", "UV_CACHE_DIR", "POETRY_CACHE_DIR"] {
        assert!(
            agent[name].starts_with(&format!("{root}/python")),
            "{name}={}",
            agent[name]
        );
        assert!(Path::new(&agent[name]).is_dir());
    }

    // The setup hook runs with the same values.
    let out = project.path().join("setup.env");
    let command = format!("env > {}", out.display());
    let outcome = kernel
        .run_setup(&command, &project.path().to_string_lossy(), 10_000)
        .unwrap();
    assert_eq!(outcome, SetupOutcome::Ok);
    let setup_env = std::fs::read_to_string(&out).unwrap();
    for name in ["PIP_CACHE_DIR", "CAPSTAN_CACHE_DIR", "NPM_CONFIG_CACHE"] {
        assert!(
            setup_env.contains(&format!("{name}={}", agent.get(name).unwrap())),
            "{name} missing in {setup_env}"
        );
    }

    // The teardown hook: cache variables next to the worktree ones, still no token or socket.
    let budget = Budget::new(Arc::new(|| 0), 60_000);
    kernel_teardown(&f, &budget);
    let teardown = seen.lock().unwrap().clone().unwrap();
    assert_eq!(teardown["NPM_CONFIG_CACHE"], agent["NPM_CONFIG_CACHE"]);
    assert_eq!(teardown["CAPSTAN_CACHE_DIR"], root);
    assert_eq!(teardown["CAPSTAN_WORKTREE_PATH"], "/tmp/wt");
    assert_eq!(teardown["CAPSTAN_AGENT_ID"], "developer-1");
    assert!(!teardown.contains_key("CAPSTAN_TOKEN"));
    assert!(!teardown.contains_key("CAPSTAN_SOCKET"));
}

fn kernel_teardown(f: &Fixture, budget: &Budget) {
    let outcome = f
        .launcher
        .kernel()
        .teardown_worktree("developer-1", "/tmp/wt", budget);
    assert_eq!(outcome, SetupOutcome::Ok);
}

#[test]
fn without_cache_the_environments_have_no_cache_names() {
    let project = tempfile::tempdir().unwrap();
    let f = fixture(project.path(), None, "/usr/bin:/bin", None);
    let kernel = f.launcher.kernel();
    assert!(kernel.cache_runtime().is_none());
    for token in [Some("token"), None] {
        let env = kernel.environment(token, Some(true)).unwrap();
        assert!(env.keys().all(|k| k != "CAPSTAN_CACHE_DIR"
            && !k.ends_with("_CACHE_DIR")
            && k != "NPM_CONFIG_CACHE"
            && k != "RUSTC_WRAPPER"));
        let mut names: Vec<&str> = env.keys().map(String::as_str).collect();
        names.sort();
        let expected: Vec<&str> = if token.is_some() {
            vec!["CAPSTAN_SOCKET", "CAPSTAN_TOKEN", "PATH"]
        } else {
            vec!["PATH"]
        };
        assert!(expected.iter().all(|n| names.contains(n)), "{names:?}");
    }
    assert!(f.events.lock().unwrap().is_empty());
}

#[test]
fn an_absent_tool_is_logged_once_and_sets_nothing() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cache = outside.path().join("root");
    let toml = cache_toml(
        &cache,
        "presets = [\"rust\"]\n[cache.rust]\nmode = \"sccache\"\n",
    );
    let empty = project.path().join("empty-bin");
    std::fs::create_dir(&empty).unwrap();
    let f = fixture(project.path(), Some(&toml), &empty.to_string_lossy(), None);
    for _ in 0..3 {
        let env = f
            .launcher
            .kernel()
            .environment(Some("token"), None)
            .unwrap();
        assert!(!env.contains_key("RUSTC_WRAPPER"));
    }
    assert_eq!(count(&f.events, "cache_tool_missing"), 1);
    assert_eq!(count(&f.events, "cache_unavailable"), 0);
}

#[test]
fn an_unwritable_root_spawns_without_cache_environment_and_logs_once() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let locked = outside.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::create_dir(locked.join("probe")).is_ok() {
        return; // running as root: permissions do not apply
    }
    let toml = cache_toml(&locked.join("cache"), "presets = [\"node\"]\n");
    let f = fixture(project.path(), Some(&toml), "/usr/bin:/bin", None);
    for _ in 0..3 {
        let env = f
            .launcher
            .kernel()
            .environment(Some("token"), None)
            .unwrap();
        assert!(!env.contains_key("CAPSTAN_CACHE_DIR"));
        assert!(!env.contains_key("NPM_CONFIG_CACHE"));
        assert!(env.contains_key("CAPSTAN_TOKEN"));
    }
    assert_eq!(count(&f.events, "cache_unavailable"), 1);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn parallel_spawns_against_one_root_all_get_the_environment() {
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let cache = outside.path().join("root");
    let toml = cache_toml(&cache, "presets = [\"python\"]\n");
    let f = Arc::new(fixture(project.path(), Some(&toml), "/usr/bin:/bin", None));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let f = Arc::clone(&f);
            std::thread::spawn(move || {
                f.launcher
                    .kernel()
                    .environment(Some("token"), None)
                    .unwrap()
            })
        })
        .collect();
    for handle in handles {
        let env = handle.join().unwrap();
        assert!(Path::new(&env["PIP_CACHE_DIR"]).is_dir());
        assert!(env.contains_key("CAPSTAN_CACHE_DIR"));
    }
    assert_eq!(count(&f.events, "cache_unavailable"), 0);
}

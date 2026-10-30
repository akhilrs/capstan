//! `cstan cache status|prune|clear` and the cache line of `cstan status`, through the real binary on a scratch git project
//! and a cache root of sparse files. No daemon runs: the commands work from capstan.toml and `.capstan/project.json`.
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

const MB: u64 = 1 << 20;
const BASE: &str = "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n";

struct Project {
    root: PathBuf,
    cache: PathBuf,
    tools: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.cache);
        let _ = std::fs::remove_dir_all(&self.tools);
    }
}

fn private(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

/// `cache` is the text after the keys of `[cache]` (`None` leaves the section out).
fn project(label: &str, cache: Option<&str>) -> Project {
    let base = capstan_testkit::short_temp_root()
        .join(format!("capstan-cachecmd-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("proj");
    let cache_dir = base.join("cache");
    let tools = base.join("tools");
    for dir in [&root, &cache_dir, &tools] {
        std::fs::create_dir_all(dir).unwrap();
    }
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(root.join("a.txt"), "a\n").unwrap();
    std::fs::write(root.join(".git/info/exclude"), ".capstan/\n").unwrap();
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "-m", "chore: initial commit"]);
    let capstan = root.join(".capstan");
    let state = capstan.join("state");
    std::fs::create_dir_all(&state).unwrap();
    private(&capstan, 0o700);
    private(&state, 0o700);
    let config = json!({
        "schemaVersion": 1, "projectId": "pcachecmd", "name": "Cache",
        "stateDirectory": state.to_string_lossy(),
        "maxSlices": 4, "maxRunMs": 3_600_000, "maxDispatches": 16,
    });
    for (name, text) in [
        ("project.json", format!("{config:#}\n")),
        ("operator.key", format!("operator-{}\n", "e".repeat(40))),
    ] {
        std::fs::write(capstan.join(name), text).unwrap();
        private(&capstan.join(name), 0o600);
    }
    let mut toml = BASE.to_string();
    if let Some(extra) = cache {
        toml.push_str(&format!(
            "[cache]\nroot = \"{}\"\npresets = [\"python\"]\nmax_size_gb = 0.1\n{extra}\n\
             [cache.entries.bundler]\ndirs = {{ BUNDLE_PATH = \"bundle\" }}\n\
             [cache.entries.ghost]\ndirs = {{ GHOST_HOME = \"g\" }}\nrequires = \"ghost-tool-that-is-not-installed\"\n",
            cache_dir.display()
        ));
    }
    std::fs::write(root.join("capstan.toml"), toml).unwrap();
    private(&root.join("capstan.toml"), 0o600);
    Project {
        root,
        cache: cache_dir,
        tools,
    }
}

impl Project {
    /// The cache root of scope project: `<cache>/projects/pcachecmd`.
    fn store(&self) -> PathBuf {
        self.cache.join("projects").join("pcachecmd")
    }

    fn fill(&self, entry: &str, bytes: u64, age_days: u64) {
        fill(&self.store().join(entry), bytes, age_days);
    }

    fn has(&self, entry: &str) -> bool {
        self.store().join(entry).is_dir()
    }

    fn cstan(&self, args: &[&str]) -> Output {
        self.cstan_env(args, &[])
    }

    fn cstan_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display());
        Command::new(env!("CARGO_BIN_EXE_cstan"))
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", path)
            .env("HOME", "/nonexistent")
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .output()
            .expect("cstan runs")
    }
}

fn fill(dir: &Path, bytes: u64, age_days: u64) {
    std::fs::create_dir_all(dir).unwrap();
    File::create(dir.join("blob"))
        .unwrap()
        .set_len(bytes)
        .unwrap();
    File::create(dir.join(".last-used"))
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(age_days * 86_400))
        .unwrap();
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn success(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}{}",
        out(output),
        err(output)
    );
    out(output)
}

#[test]
fn status_prints_every_entry_with_its_size_and_the_reason_an_absent_tool_is_off() {
    let p = project("status", Some(""));
    p.fill("python", 3 * MB, 2);
    p.fill("bundler", 5 * MB, 1);
    let text = success(&p.cstan(&["cache", "status"]));
    assert!(text.contains("cache root:"), "{text}");
    let row = |name: &str| {
        text.lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_else(|| panic!("{name} in {text}"))
    };
    assert!(row("python").contains("3 MB"), "{text}");
    assert!(row("bundler").contains("5 MB"), "{text}");
    assert!(
        row("ghost").contains("off") && row("ghost").contains("ghost-tool-that-is-not-installed"),
        "{text}"
    );
    assert!(text.contains("total 8 MB of 102.4 MB"), "{text}");
    assert!(err(&p.cstan(&["cache", "status"])).contains("warning: cache entry ghost is off"));
}

#[test]
fn status_json_has_the_documented_shape() {
    let p = project("json", Some(""));
    p.fill("python", 3 * MB, 2);
    let value: Value =
        serde_json::from_str(&success(&p.cstan(&["cache", "status", "--json"]))).unwrap();
    for key in [
        "schemaVersion",
        "root",
        "scope",
        "maxBytes",
        "maxAgeDays",
        "totalBytes",
        "partial",
        "measuredAt",
        "entries",
        "setupSnapshots",
        "warnings",
    ] {
        assert!(value.get(key).is_some(), "{key} in {value}");
    }
    assert_eq!(value["scope"], "project");
    assert_eq!(value["totalBytes"], 3 * MB);
    let python = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "python")
        .unwrap();
    for key in [
        "name",
        "origin",
        "enabled",
        "reason",
        "mode",
        "bytes",
        "files",
        "lastUsedAt",
        "partial",
        "dir",
    ] {
        assert!(python.get(key).is_some(), "{key} in {python}");
    }
    assert_eq!(python["bytes"], 3 * MB);
    assert_eq!(python["enabled"], true);
    let ghost = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "ghost")
        .unwrap();
    assert_eq!(ghost["enabled"], false);
    assert!(ghost["reason"]
        .as_str()
        .unwrap()
        .contains("not on the agent PATH"));
}

#[test]
fn the_cache_commands_need_a_cache_section() {
    let p = project("none", None);
    let output = p.cstan(&["cache", "status"]);
    assert_eq!(output.status.code(), Some(3), "{}", err(&output));
    assert!(err(&output).contains("no [cache] section"));
    let status: Value = serde_json::from_str(&success(&p.cstan(&["status", "--json"]))).unwrap();
    assert!(status.get("cache").is_none(), "{status}");
    assert!(!success(&p.cstan(&["status"])).contains("cache:"));
}

#[test]
fn status_has_one_cache_line_and_a_cache_member_without_a_daemon() {
    let p = project("line", Some(""));
    p.fill("python", 2 * MB, 1);
    // No measurement yet: the member lists the entries without sizes.
    let text = success(&p.cstan(&["status"]));
    let line = text.lines().find(|l| l.starts_with("cache:")).expect(&text);
    assert!(
        line.contains("python not measured")
            && line
                .contains("ghost off (ghost-tool-that-is-not-installed is not on the agent PATH)"),
        "{line}"
    );
    assert!(line.contains("not measured yet"), "{line}");
    // A prune writes the manifest; status reads it and never walks the disk.
    success(&p.cstan(&["cache", "prune"]));
    let text = success(&p.cstan(&["status"]));
    let line = text.lines().find(|l| l.starts_with("cache:")).expect(&text);
    assert!(
        line.contains("python 2 MB")
            && line.contains("total 2 of 102.4 MB")
            && line.contains("measured "),
        "{line}"
    );
    let status: Value = serde_json::from_str(&success(&p.cstan(&["status", "--json"]))).unwrap();
    assert_eq!(status["cache"]["totalBytes"], 2 * MB);
    assert_eq!(status["cache"]["scope"], "project");
}

#[test]
fn clear_removes_only_the_named_entry() {
    let p = project("clear", Some(""));
    p.fill("python", MB, 1);
    p.fill("bundler", MB, 1);
    let text = success(&p.cstan(&["cache", "clear", "python"]));
    assert!(text.contains("cleared python"), "{text}");
    assert!(!p.has("python") && p.has("bundler"));
    let missing = p.cstan(&["cache", "clear", "nope"]);
    assert_eq!(missing.status.code(), Some(3), "{}", err(&missing));
    assert_eq!(p.cstan(&["cache", "clear"]).status.code(), Some(3));
}

#[test]
fn clear_all_is_refused_while_a_worktree_exists_without_force() {
    let p = project("all", Some(""));
    p.fill("python", MB, 1);
    p.fill("bundler", MB, 1);
    let worktree = p.cache.join("wt");
    git(
        &p.root,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feat/w",
            worktree.to_str().unwrap(),
        ],
    );
    let refused = p.cstan(&["cache", "clear", "--all"]);
    assert_eq!(refused.status.code(), Some(4), "{}", err(&refused));
    assert!(err(&refused).contains("--force"));
    assert!(p.has("python") && p.has("bundler"));
    let text = success(&p.cstan(&["cache", "clear", "--all", "--force"]));
    assert!(
        text.contains("cleared python") && text.contains("cleared bundler"),
        "{text}"
    );
    assert!(!p.has("python") && !p.has("bundler"));
}

#[test]
fn clear_of_scope_user_needs_force() {
    let p = project("user", Some("scope = \"user\""));
    let shared = p.cache.join("shared");
    fill(&shared.join("python"), MB, 1);
    let refused = p.cstan(&["cache", "clear", "python"]);
    assert_eq!(refused.status.code(), Some(4), "{}", err(&refused));
    assert!(err(&refused).contains("scope user"));
    assert!(shared.join("python").is_dir());
    success(&p.cstan(&["cache", "clear", "python", "--force"]));
    assert!(!shared.join("python").exists());
}

#[test]
fn a_dry_run_prune_lists_what_would_go_and_changes_nothing() {
    let p = project("dry", Some(""));
    p.fill("python", 80 * MB, 40);
    p.fill("bundler", 80 * MB, 1);
    let text = success(&p.cstan(&["cache", "prune", "--dry-run"]));
    assert!(text.contains("dry run"), "{text}");
    assert!(text.contains("would clear python"), "{text}");
    assert!(p.has("python") && p.has("bundler"));
    // The measurement may be refreshed, but a dry run never records a prune.
    if let Ok(manifest) = std::fs::read_to_string(p.store().join("manifest.json")) {
        assert!(manifest.contains("\"lastPruneAt\": null"), "{manifest}");
    }
    let value: Value = serde_json::from_str(&success(&p.cstan(&[
        "cache",
        "prune",
        "--dry-run",
        "--json",
    ])))
    .unwrap();
    assert_eq!(value["dryRun"], true);
    assert_eq!(value["cleared"], json!([]));
}

#[test]
fn prune_over_the_cap_clears_the_least_recently_used() {
    let p = project("prune", Some(""));
    p.fill("python", 60 * MB, 5);
    p.fill("bundler", 60 * MB, 1);
    let text = success(&p.cstan(&["cache", "prune"]));
    assert!(text.contains("cleared python"), "{text}");
    assert!(!p.has("python") && p.has("bundler"));
}

#[test]
fn prune_runs_a_command_found_on_path_with_only_the_allowlisted_environment() {
    let p = project(
        "env",
        Some("\n[cache.entries.npmlike]\ndirs = { NPM_X = \"x\" }\nprune = [\"fakenpm cache verify\"]\n"),
    );
    let stub = p.tools.join("fakenpm");
    std::fs::write(&stub, "#!/bin/sh\nenv > ./env.txt\n").unwrap();
    private(&stub, 0o755);
    p.fill("npmlike", MB, 1);
    let output = p.cstan_env(
        &["cache", "prune"],
        &[("SECRET_TOKEN", "hunter2"), ("LANG", "C")],
    );
    let text = success(&output);
    assert!(
        text.contains("ran npmlike: fakenpm cache verify: ok"),
        "{text}"
    );
    let seen = std::fs::read_to_string(p.store().join("npmlike").join("env.txt"))
        .unwrap_or_else(|e| panic!("env.txt: {e}; {text}"));
    let names: Vec<&str> = seen
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, _)| k))
        .collect();
    let allowed = [
        "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "TERM", "TMPDIR", "NPM_X", "PWD",
        "OLDPWD", "SHLVL", "_",
    ];
    for name in &names {
        assert!(allowed.contains(name), "{name} leaked: {seen}");
    }
    assert!(
        names.contains(&"PATH") && names.contains(&"NPM_X"),
        "{seen}"
    );
    assert!(!seen.contains("hunter2"), "{seen}");
}

#[test]
fn the_setup_entry_is_listed_and_clearing_one_snapshot_removes_only_it() {
    let p = project(
        "setup",
        Some("\n[cache.setup_reuse]\nkey_files = [\"a.txt\"]\npaths = [\"target\"]\n"),
    );
    fill(&p.store().join("setup").join("k1"), 3 * MB, 1);
    fill(&p.store().join("setup").join("k2"), 4 * MB, 1);
    let value: Value =
        serde_json::from_str(&success(&p.cstan(&["cache", "status", "--json"]))).unwrap();
    let setup = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "setup")
        .unwrap();
    assert_eq!(setup["bytes"], 7 * MB);
    assert_eq!(value["setupSnapshots"].as_array().unwrap().len(), 2);
    success(&p.cstan(&["cache", "clear", "setup/k1"]));
    assert!(
        !p.store().join("setup").join("k1").exists() && p.store().join("setup").join("k2").is_dir()
    );
    let value: Value =
        serde_json::from_str(&success(&p.cstan(&["cache", "status", "--json"]))).unwrap();
    let setup = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "setup")
        .unwrap();
    assert_eq!(setup["bytes"], 4 * MB);
    // The manifest was measured again after the clear: the status line shows the setup entry's size.
    let text = success(&p.cstan(&["status"]));
    assert!(text.contains("setup 4 MB"), "{text}");
    success(&p.cstan(&["cache", "clear", "setup"]));
    assert!(!p.has("setup"));
}

/// A stand-in daemon on the project's control socket: answers one request with `reply` and returns the request frame.
fn fake_daemon(p: &Project, reply: Value) -> std::thread::JoinHandle<Value> {
    let socket = p.root.join(".capstan").join("state").join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let mut writer = &stream;
        writeln!(writer, "{reply}").unwrap();
        serde_json::from_str(&line).unwrap()
    })
}

#[test]
fn an_agent_environment_sends_prune_and_clear_to_the_daemon_and_status_stays_local() {
    let p = project("agent", Some(""));
    p.fill("python", MB, 1);
    let socket = p.root.join(".capstan").join("state").join("control.sock");
    let env = [
        ("CAPSTAN_TOKEN", "agent-token"),
        ("CAPSTAN_SOCKET", socket.to_str().unwrap()),
    ];
    let reply = json!({"ok": true, "result": {"cleared": ["python"], "stopCommands": [], "errors": [], "totalBytes": 0}});
    let daemon = fake_daemon(&p, reply);
    let text = success(&p.cstan_env(&["cache", "clear", "python", "--force"], &env));
    let frame = daemon.join().unwrap();
    assert_eq!(frame["command"], "cache");
    assert_eq!(frame["credential"], "agent-token");
    assert_eq!(frame["args"], json!(["clear", "python", "--force"]));
    assert!(text.contains("cleared python"), "{text}");
    // The daemon answered; nothing ran locally.
    assert!(p.has("python"));
    let _ = std::fs::remove_file(&socket);

    let refusal = json!({"ok": false, "code": "forbidden", "message": "only the PM or the operator may prune or clear the cache"});
    let daemon = fake_daemon(&p, refusal);
    let output = p.cstan_env(&["cache", "prune"], &env);
    let frame = daemon.join().unwrap();
    assert_eq!(frame["args"], json!(["prune"]));
    assert_eq!(output.status.code(), Some(4), "{}", err(&output));
    assert!(err(&output).contains("only the PM or the operator"));
    let _ = std::fs::remove_file(&socket);

    // cache status never goes to the daemon.
    let text = success(&p.cstan_env(&["cache", "status"], &env));
    assert!(text.contains("python"), "{text}");
}

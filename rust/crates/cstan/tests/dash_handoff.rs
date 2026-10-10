//! `cstan dash` hands over to `cstan-dash`: where it looks, what it passes and what it says when there is none. This path
//! has no Node counterpart (Node ran its own dashboard when it found no `cstan-dash`), so no transcript reaches it: the
//! cases are here, with a stub `cstan-dash` that prints its arguments.
mod support;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cstan_front::{run, Context, ExecPlan, Outcome};
use serde_json::json;
use support::*;

const MISSING_LINE: &str = "cstan: cstan-dash was not found; install it beside cstan, put it on PATH or set CSTAN_DASH_BIN to its absolute path (docs/reference/install.md)\n";

struct Project {
    scratch: Scratch,
    _daemon: FakeDaemon,
}

impl Project {
    fn path(&self, relative: &str) -> PathBuf {
        self.scratch.0.join(relative)
    }
}

/// A scratch project with a daemon that answers every request with a ping reply.
fn project(label: &str, index: usize, with_toml: bool) -> Project {
    let scratch = Scratch::new(label, index);
    let root = scratch.text();
    let mut contents = serde_json::Map::new();
    contents.insert(
        "proj/.capstan/operator.key".into(),
        json!("fixture-operator-key-0123456789abcdefghijklmnopqrstuvwxyz\n"),
    );
    contents.insert(
        "proj/.capstan/project.json".into(),
        json!(format!(
            "{{\n  \"schemaVersion\": 1,\n  \"projectId\": \"pfixture\",\n  \"name\": \"proj\",\n  \"stateDirectory\": \"{root}/proj/.capstan/state\",\n  \"maxSlices\": 4,\n  \"maxRunMs\": 3600000,\n  \"maxDispatches\": 16\n}}\n"
        )),
    );
    if with_toml {
        contents.insert(
            "proj/capstan.toml".into(),
            json!(capstan_config::starter::STARTER_CONFIG),
        );
        contents.insert(
            format!("proj/{}", capstan_config::starter::DESIGNER_PROMPT_PATH),
            json!(capstan_config::starter::DESIGNER_PROMPT),
        );
    }
    let layout = json!({
        "dirs": ["proj/.capstan/state", "bin"],
        "files": [],
        "contents": contents,
        "modes": {"proj/.capstan": "0700", "proj/.capstan/state": "0700"},
    });
    make_layout(&layout, &scratch.0, &root);
    let daemon = FakeDaemon::start(
        &scratch.0.join("proj/.capstan/state/control.sock"),
        &json!({"kind": "line", "text": "{\"ok\":true,\"result\":{\"pong\":true,\"pid\":4242}}"}),
    );
    Project {
        scratch,
        _daemon: daemon,
    }
}

fn write_stub(path: &Path, version_line: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{version_line}'; exit 0; fi\nprintf '%s\\n' \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn context(project: &Project, args: &[&str], env: &[(&str, String)], cli_path: PathBuf) -> Context {
    Context {
        args: args.iter().map(OsString::from).collect(),
        env: env
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect(),
        cwd: project.path("proj"),
        now_ms: 0.0,
        cli_path,
        terminal: Some(true),
    }
}

fn exec_plan(outcome: Outcome) -> ExecPlan {
    match outcome {
        Outcome::Exec { plan, .. } => plan,
        other => panic!("not a hand-over: {other:?}"),
    }
}

fn base_env(project: &Project) -> Vec<(&'static str, String)> {
    vec![
        ("PATH", project.path("empty").to_string_lossy().into_owned()),
        ("HOME", project.path("home").to_string_lossy().into_owned()),
        ("CAPSTAN_LAUNCH", "off".to_string()),
    ]
}

#[test]
fn no_cstan_dash_is_one_line_and_the_usage_exit_code() {
    let project = project("dash-none", 0, false);
    let cli = project.path("bin/cstan");
    let outcome = run(&context(&project, &["dash"], &base_env(&project), cli));
    let Outcome::Done {
        stdout,
        stderr,
        exit,
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert_eq!(String::from_utf8(stderr).unwrap(), MISSING_LINE);
    assert!(stdout.is_empty());
    assert_eq!(exit, 2);
}

#[test]
fn a_file_that_is_not_cstan_dash_is_not_run_as_one() {
    let project = project("dash-fake", 1, false);
    let stub = project.path("bin/cstan-dash");
    write_stub(&stub, "something else 1.0");
    let outcome = run(&context(
        &project,
        &["dash"],
        &base_env(&project),
        project.path("bin/cstan"),
    ));
    let Outcome::Done { stderr, exit, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(String::from_utf8(stderr).unwrap(), MISSING_LINE);
    assert_eq!(exit, 2);
}

fn args_of(plan: &ExecPlan) -> Vec<String> {
    plan.args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn socket_of(project: &Project) -> String {
    project
        .path("proj/.capstan/state/control.sock")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn cstan_dash_beside_cstan_gets_the_socket_interval_and_credential() {
    let project = project("dash-beside", 2, false);
    let stub = project.path("bin/cstan-dash");
    write_stub(&stub, "cstan-dash 9.9.9");
    let mut env = base_env(&project);
    env.push(("CAPSTAN_ALLOW_FOREIGN_SOCKET", "1".into()));
    let plan = exec_plan(run(&context(
        &project,
        &["dash"],
        &env,
        project.path("bin/cstan"),
    )));
    assert_eq!(plan.program, stub);
    assert_eq!(
        args_of(&plan),
        ["--socket", &socket_of(&project), "--interval", "2"]
    );
    let credential = plan
        .env
        .iter()
        .find(|(k, _)| k == "CSTAN_DASH_CREDENTIAL")
        .map(|(_, v)| v.to_string_lossy().into_owned());
    assert_eq!(
        credential.as_deref(),
        Some("fixture-operator-key-0123456789abcdefghijklmnopqrstuvwxyz")
    );
}

#[test]
fn the_flags_and_the_worker_limit_pass_through_and_the_agent_credentials_do_not() {
    let project = project("dash-flags", 3, true);
    let stub = project.path("bin/cstan-dash");
    write_stub(&stub, "cstan-dash 9.9.9");
    let mut env = base_env(&project);
    env.push(("CSTAN_REDUCED_MOTION", "1".into()));
    let plan = exec_plan(run(&context(
        &project,
        &["dash", "--interval", "7", "--no-color"],
        &env,
        project.path("bin/cstan"),
    )));
    assert_eq!(
        args_of(&plan),
        [
            "--socket",
            &socket_of(&project),
            "--interval",
            "7",
            "--worker-limit",
            "3",
            "--no-color",
            "--reduced-motion"
        ]
    );
    assert!(plan
        .env
        .iter()
        .all(|(k, _)| k != "CAPSTAN_TOKEN" && k != "CAPSTAN_SOCKET"));
}

#[test]
fn cstan_dash_bin_wins_over_beside_and_path() {
    let project = project("dash-override", 4, false);
    write_stub(&project.path("bin/cstan-dash"), "cstan-dash 1.0.0");
    let chosen = project.path("elsewhere/cstan-dash");
    write_stub(&chosen, "cstan-dash 2.0.0");
    let mut env = base_env(&project);
    env.push(("CSTAN_DASH_BIN", chosen.to_string_lossy().into_owned()));
    let plan = exec_plan(run(&context(
        &project,
        &["dash"],
        &env,
        project.path("bin/cstan"),
    )));
    assert_eq!(plan.program, chosen);
}

#[test]
fn cstan_dash_on_path_is_found_last() {
    let project = project("dash-path", 5, false);
    let on_path = project.path("tools/cstan-dash");
    write_stub(&on_path, "cstan-dash 3.0.0");
    let mut env = base_env(&project);
    env.retain(|(k, _)| *k != "PATH");
    env.push(("PATH", project.path("tools").to_string_lossy().into_owned()));
    // The executable is in a directory with no cstan-dash beside it.
    let plan = exec_plan(run(&context(
        &project,
        &["dash"],
        &env,
        project.path("home/cstan"),
    )));
    assert_eq!(plan.program, on_path);
}

#[test]
fn cstan_dash_rust_demands_a_binary() {
    let project = project("dash-rust", 6, false);
    let mut env = base_env(&project);
    env.push(("CSTAN_DASH", "rust".into()));
    let outcome = run(&context(
        &project,
        &["dash"],
        &env,
        project.path("bin/cstan"),
    ));
    let Outcome::Done { stderr, exit, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(
        String::from_utf8(stderr).unwrap(),
        "cstan: CSTAN_DASH=rust but no executable cstan-dash was found; set CSTAN_DASH_BIN to its absolute path or install it (docs/reference/install.md)\n"
    );
    assert_eq!(exit, 5);
}

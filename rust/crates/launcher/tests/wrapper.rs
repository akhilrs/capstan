//! The `cstan` wrapper the launcher puts first on every agent's PATH (replaces test/launcher-wrapper.test.ts for the
//! Rust launcher): its exact text, the Node variables it passes on, and what it does with paths and arguments when an
//! agent shell runs it. Nothing here needs Node: the `cstan` it runs is a shell stub that records what it was given.
use capstan_launcher::shared::{cstan_wrapper_script, FrontEndSite};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn site(cstan: &str, env: &[(&str, &str)]) -> FrontEndSite {
    FrontEndSite {
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
        exec_path: "/opt/capstan/cstan-node".into(),
        sea: true,
        cstan: cstan.into(),
    }
}

fn executable(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn without_node_variables_the_wrapper_only_runs_the_cstan_it_was_given() {
    assert_eq!(
        cstan_wrapper_script(&site("/opt/capstan/cstan", &[])),
        "#!/bin/sh\nexec '/opt/capstan/cstan' \"$@\"\n"
    );
}

#[test]
fn the_node_variables_are_passed_on_only_when_the_daemon_has_them() {
    assert_eq!(
        cstan_wrapper_script(&site(
            "/opt/capstan/cstan",
            &[
                ("CSTAN_NODE_CLI", "/opt/capstan/cli.js"),
                ("CSTAN_NODE", "/usr/bin/node")
            ]
        )),
        "#!/bin/sh\nCSTAN_NODE_CLI='/opt/capstan/cli.js' CSTAN_NODE='/usr/bin/node' exec '/opt/capstan/cstan' \"$@\"\n"
    );
    // One of them, and an empty one, which counts as absent.
    assert_eq!(
        cstan_wrapper_script(&site(
            "/opt/capstan/cstan",
            &[
                ("CSTAN_NODE_CLI", "/opt/capstan/cli.js"),
                ("CSTAN_NODE", "")
            ]
        )),
        "#!/bin/sh\nCSTAN_NODE_CLI='/opt/capstan/cli.js' exec '/opt/capstan/cstan' \"$@\"\n"
    );
    // Variables that are not the two are never copied into the script.
    assert_eq!(
        cstan_wrapper_script(&site(
            "/c",
            &[("CAPSTAN_TOKEN", "secret"), ("PATH", "/bin")]
        )),
        "#!/bin/sh\nexec '/c' \"$@\"\n"
    );
}

#[test]
fn the_wrapper_quotes_paths_and_passes_the_arguments_and_variables_through() {
    let scratch = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(scratch.path()).unwrap();
    let out = root.join("wrapper-out");
    let front = root.join("it's here").join("cstan");
    executable(
        &front,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$CSTAN_NODE_CLI\" \"$CSTAN_NODE\" \"$#\" \"$@\" > '{}'\n",
            out.display()
        ),
    );
    let cli = root.join("dist dir").join("cli.js");
    let wrapper = root.join("wrapper.sh");
    executable(
        &wrapper,
        &cstan_wrapper_script(&site(
            &front.to_string_lossy(),
            &[
                ("CSTAN_NODE_CLI", &cli.to_string_lossy()),
                ("CSTAN_NODE", "/opt/n o/node"),
            ],
        )),
    );
    let status = Command::new(&wrapper)
        .args(["ping", "--json", "a b", ""])
        .env_clear()
        .status()
        .unwrap();
    assert!(status.success());
    let seen = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = seen.split('\n').collect();
    assert_eq!(
        lines[..7],
        [
            cli.to_str().unwrap(),
            "/opt/n o/node",
            "4",
            "ping",
            "--json",
            "a b",
            ""
        ]
    );
}

#[test]
fn the_wrapper_does_not_take_node_variables_from_the_agent_shell() {
    // The variables in the script are the daemon's; whatever the agent's shell holds is overwritten by them.
    let scratch = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(scratch.path()).unwrap();
    let out = root.join("seen");
    let front = root.join("cstan");
    executable(
        &front,
        &format!(
            "#!/bin/sh\nprintf '%s' \"$CSTAN_NODE_CLI\" > '{}'\n",
            out.display()
        ),
    );
    let wrapper = root.join("wrapper.sh");
    executable(
        &wrapper,
        &cstan_wrapper_script(&site(
            &front.to_string_lossy(),
            &[("CSTAN_NODE_CLI", "/daemon/cli.js")],
        )),
    );
    assert!(Command::new(&wrapper)
        .env_clear()
        .env("CSTAN_NODE_CLI", "/agent/other.js")
        .status()
        .unwrap()
        .success());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "/daemon/cli.js");
}

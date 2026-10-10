//! The `cstan` wrapper the launcher puts first on every agent's PATH (replaces test/launcher-wrapper.test.ts for the
//! Rust launcher): its exact text, and what it does with paths and arguments when an
//! agent shell runs it. Nothing here needs Node: the `cstan` it runs is a shell stub that records what it was given.
mod common;

use capstan_launcher::shared::{cstan_wrapper_script, FrontEndSite};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

fn site(cstan: &str, env: &[(&str, &str)]) -> FrontEndSite {
    FrontEndSite {
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
        exec_path: "/opt/capstan/cstan".into(),
        sea: true,
        cstan: cstan.into(),
    }
}

fn executable(path: &Path, text: &str) {
    common::exec::write_script(path, text, 0o755);
}

#[test]
fn the_wrapper_only_runs_the_cstan_it_was_given() {
    assert_eq!(
        cstan_wrapper_script(&site("/opt/capstan/cstan", &[])),
        "#!/bin/sh\nexec '/opt/capstan/cstan' \"$@\"\n"
    );
}

#[test]
fn no_variable_of_the_daemon_is_copied_into_the_script() {
    // Whatever the daemon's environment holds, the script only runs the cstan it was given.
    assert_eq!(
        cstan_wrapper_script(&site(
            "/c",
            &[
                ("CAPSTAN_TOKEN", "secret"),
                ("PATH", "/bin"),
                ("CSTAN_NODE_CLI", "/opt/capstan/cli.js"),
            ]
        )),
        "#!/bin/sh\nexec '/c' \"$@\"\n"
    );
}

#[test]
fn the_wrapper_quotes_paths_and_passes_the_arguments_through() {
    let _exec = common::exec::exclusive();
    let scratch = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(scratch.path()).unwrap();
    let out = root.join("wrapper-out");
    let front = root.join("it's here").join("cstan");
    executable(
        &front,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$@\" > '{}'\n",
            out.display()
        ),
    );
    let wrapper = root.join("wrapper.sh");
    executable(
        &wrapper,
        &cstan_wrapper_script(&site(&front.to_string_lossy(), &[])),
    );
    let status = Command::new(&wrapper)
        .args(["ping", "--json", "a b", ""])
        .env_clear()
        .status()
        .unwrap();
    assert!(status.success());
    let seen = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = seen.split('\n').collect();
    assert_eq!(lines[..5], ["4", "ping", "--json", "a b", ""]);
}

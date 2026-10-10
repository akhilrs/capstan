//! An agent shell runs `cstan config check` through the wrapper the launcher writes, and gets the output of the frozen
//! transcript (crates/cstan/tests/transcripts/config-check-starter.json). The wrapper runs the Rust `cstan`, which hands
//! the command to the Node CLI named by `CSTAN_NODE_CLI` until the commands are native.
//!
//! Ignored: it needs a built `cstan` and the Node CLI. Run by hand, after `cargo build -p cstan-front`:
//! `CSTAN_NODE_CLI=<repo>/dist/src/cli.js cargo test -p capstan-launcher --test wrapper_handoff -- --ignored`
//! (`CSTAN_NODE` names the node binary if it is not `node` on PATH; `CSTAN_BIN` the `cstan` if it is not beside the
//! test build). Plan B's cutover makes it a normal test.
use capstan_launcher::shared::{cstan_wrapper_script, FrontEndSite};
use serde_json::Value;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cstan_binary() -> PathBuf {
    if let Some(named) = std::env::var_os("CSTAN_BIN") {
        return PathBuf::from(named);
    }
    // target/<profile>/deps/<this test> -> target/<profile>/cstan
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().join("cstan")
}

fn write_executable(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
#[ignore = "needs a built cstan and CSTAN_NODE_CLI (the dist build of the Node CLI); run by hand"]
fn an_agent_shell_runs_config_check_through_the_wrapper() {
    let cli = std::env::var("CSTAN_NODE_CLI").expect("set CSTAN_NODE_CLI to the dist build");
    let cstan = cstan_binary();
    assert!(
        cstan.is_file(),
        "no cstan at {}: run cargo build -p cstan-front",
        cstan.display()
    );
    let transcript: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../cstan/tests/transcripts/config-check-starter.json"),
        )
        .unwrap(),
    )
    .unwrap();

    let scratch = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(scratch.path()).unwrap();
    for dir in transcript["layout"]["dirs"].as_array().unwrap() {
        std::fs::create_dir_all(root.join(dir.as_str().unwrap())).unwrap();
    }
    for (file, text) in transcript["layout"]["contents"].as_object().unwrap() {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text.as_str().unwrap()).unwrap();
    }

    // The wrapper exactly as the daemon's launcher writes it, for a daemon that has the Node variables.
    let mut env: HashMap<String, String> = HashMap::new();
    env.insert("CSTAN_NODE_CLI".into(), cli);
    if let Ok(node) = std::env::var("CSTAN_NODE") {
        env.insert("CSTAN_NODE".into(), node);
    }
    let site = FrontEndSite {
        env,
        exec_path: cstan.to_string_lossy().into_owned(),
        sea: true,
        cstan: cstan.to_string_lossy().into_owned(),
    };
    let bin = root.join("agent-bin");
    write_executable(&bin.join("cstan"), &cstan_wrapper_script(&site));

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("sh")
        .arg("-c")
        .arg("cstan config check")
        .current_dir(root.join(transcript["cwd"].as_str().unwrap()))
        .env_clear()
        .env("PATH", path)
        .env("TZ", "UTC")
        .output()
        .unwrap();

    let expected = &transcript["node"];
    let fill = |text: &str| text.replace("$ROOT", &root.to_string_lossy());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        fill(expected["stdout"].as_str().unwrap())
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        fill(expected["stderr"].as_str().unwrap())
    );
    assert_eq!(
        i64::from(output.status.code().unwrap()),
        expected["exit"].as_i64().unwrap()
    );
}

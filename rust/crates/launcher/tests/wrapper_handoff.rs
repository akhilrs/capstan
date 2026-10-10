//! An agent shell runs `cstan config check` through the wrapper the launcher writes, and gets the output of the frozen
//! transcript (crates/cstan/tests/transcripts/config-check-starter.json). The wrapper runs the Rust `cstan`, which serves
//! `config check` itself; Node is neither needed nor on the path the shell gets.
//!
//! It needs the built `cstan` (`cargo test` of the workspace builds it; for this package alone run
//! `cargo build -p cstan-front` first; `CSTAN_BIN` names it if it is not beside the test build).
mod common;

use capstan_launcher::shared::{cstan_wrapper_script, FrontEndSite};
use serde_json::Value;
use std::collections::HashMap;
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
    common::exec::write_script(path, text, 0o700);
}

#[test]
fn an_agent_shell_runs_config_check_through_the_wrapper() {
    let _exec = common::exec::exclusive();
    let cstan = cstan_binary();
    assert!(
        cstan.is_file(),
        "no cstan at {}: run cargo build -p cstan-front (cargo test of the workspace builds it)",
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

    // The wrapper exactly as the daemon's launcher writes it,
    let site = FrontEndSite {
        env: HashMap::new(),
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

//! `scripts/trust-dry-run.sh` with a stub `cargo` on PATH: nothing here builds or tests anything for real.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../scripts/trust-dry-run.sh")
}

fn rust_dir() -> PathBuf {
    fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

/// A stub `cargo`. `build_body` runs for `build`; the `test` step writes its environment and arguments to `$OUT`.
fn stub(dir: &Path, build_body: &str) -> PathBuf {
    let bin = dir.join("bin");
    fs::create_dir(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(
        &cargo,
        format!(
            r#"#!/bin/sh
case "$1" in
  build) {build_body} ;;
  test)
    {{
      echo "DRY=$CAPSTAN_TRUST_DRY_RUN"
      echo "DIR=$CAPSTAN_TRUST_DIR"
      echo "ENFORCE=$CAPSTAN_TRUST_ENFORCE"
      echo "BIN=$CSTAN_BIN"
      echo "ARGS=$*"
      echo "MODE=$(stat -c %a "$CAPSTAN_TRUST_DIR")"
    }} > "$OUT"
    ;;
esac
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn run(bin: &Path, out: &Path, target: Option<&str>, set_dry_run: bool) -> Output {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let mut command = Command::new("bash");
    command
        .arg(script())
        .args(["-p", "x", "--", "y"])
        .env("PATH", path)
        .env("OUT", out)
        .env_remove("CAPSTAN_TRUST_DRY_RUN")
        .env_remove("CAPSTAN_TRUST_ENFORCE")
        .env_remove("CAPSTAN_TRUST_DIR")
        .env_remove("CARGO_TARGET_DIR");
    if let Some(target) = target {
        command.env("CARGO_TARGET_DIR", target);
    }
    if set_dry_run {
        command.env("CAPSTAN_TRUST_DRY_RUN", "1");
    }
    command.output().unwrap()
}

fn lines(out: &Path) -> Vec<String> {
    fs::read_to_string(out)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn it_refuses_inside_a_dry_run() {
    let tmp = TempDir::new().unwrap();
    let bin = stub(tmp.path(), ":");
    let out = tmp.path().join("out");
    let result = run(&bin, &out, None, true);
    assert!(!result.status.success());
    assert!(!out.exists());
}

#[test]
fn it_passes_the_gate_environment_to_the_test_step() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("target");
    let bin = stub(
        tmp.path(),
        &format!(
            "mkdir -p {0}/debug && : > {0}/debug/cstan && chmod 755 {0}/debug/cstan",
            target.display()
        ),
    );
    let out = tmp.path().join("out");
    let result = run(&bin, &out, Some(target.to_str().unwrap()), false);
    assert!(result.status.success(), "{result:?}");
    let got = lines(&out);
    assert_eq!(got[0], "DRY=1");
    assert!(got[1].starts_with("DIR=/") && got[1].len() > 5, "{got:?}");
    assert_eq!(got[2], "ENFORCE=1");
    assert_eq!(got[3], format!("BIN={}/debug/cstan", target.display()));
    assert_eq!(got[4], "ARGS=test --no-fail-fast -p x -- y");
    assert_eq!(got[5], "MODE=700");
    let dir = got[1].trim_start_matches("DIR=");
    assert!(!Path::new(dir).exists(), "the temp dir is removed on exit");
}

#[test]
fn it_fails_before_testing_without_a_binary_or_after_a_failed_build() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    let target = tmp.path().join("target");
    let nothing = stub(tmp.path(), ":");
    let result = run(&nothing, &out, Some(target.to_str().unwrap()), false);
    assert!(!result.status.success());
    assert!(!out.exists());

    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("out");
    let target = tmp.path().join("target");
    let failing = stub(
        tmp.path(),
        &format!(
            "mkdir -p {0}/debug && : > {0}/debug/cstan && chmod 755 {0}/debug/cstan; exit 1",
            target.display()
        ),
    );
    let result = run(&failing, &out, Some(target.to_str().unwrap()), false);
    assert!(!result.status.success());
    assert!(!out.exists());
}

#[test]
fn the_target_dir_defaults_to_rust_target_and_a_relative_one_resolves_against_rust() {
    let rust = rust_dir();
    for (set, expected) in [
        (None, rust.join("target")),
        (Some("rel-target"), rust.join("rel-target")),
    ] {
        let tmp = TempDir::new().unwrap();
        // The stub records the path the script expects and creates nothing, so the script stops after the build.
        let bin = stub(tmp.path(), ":");
        let out = tmp.path().join("out");
        let result = run(&bin, &out, set, false);
        let want = format!("{}/debug/cstan", expected.display());
        if result.status.success() {
            // A binary was already built there: the test step saw exactly that path.
            assert!(lines(&out).contains(&format!("BIN={want}")));
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.contains(&want), "{stderr}");
        }
    }
}

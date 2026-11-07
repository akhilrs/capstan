//! `cstan init` trusts the starter `capstan.toml` it writes, and nothing else.
mod support;

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use capstan_config::{
    checked_subset, read_config_pair, record_current, trust_status_with, TrustStatus,
};
use support::*;

const TRUSTED: &str = "Trusted capstan.toml on this machine; later changes need cstan trust.";

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.com",
        ])
        .args(args)
        .env_clear()
        .env("PATH", HOST_PATH)
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_new_folder_is_trusted_and_loads_without_cstan_trust() {
    let bed = TrustBed::new("init-new");
    let (exit, out, _) = bed.run(&["init"], bed.env());
    assert_eq!(exit, 0, "{out}");
    assert!(out.contains(TRUSTED), "{out}");
    assert_eq!(
        trust_status_with(&bed.project, &bed.store()),
        TrustStatus::Trusted
    );
    let (exit, _, err) = bed.run(&["config", "check"], bed.env());
    assert_eq!(exit, 0, "{err}");
    let (exit, out, _) = bed.run(&["trust", "--check"], bed.env());
    assert_eq!(
        (exit, out.as_str()),
        (0, "capstan.toml is trusted on this machine.\n")
    );
}

#[test]
fn an_existing_capstan_toml_is_never_trusted_by_init() {
    let bed = TrustBed::new("init-existing");
    bed.write(TRUST_FIXTURE_TOML);
    let (exit, out, _) = bed.run(&["init"], bed.env());
    assert_eq!(exit, 0, "{out}");
    assert!(
        out.contains("capstan.toml exists: review it with cstan trust.\n"),
        "{out}"
    );
    assert!(!out.contains(TRUSTED));
    assert!(!bed.store_dir.exists());
    assert!(matches!(
        trust_status_with(&bed.project, &bed.store()),
        TrustStatus::Needed { .. }
    ));
}

#[test]
fn an_unavailable_store_does_not_fail_init() {
    let bed = TrustBed::new("init-unavailable");
    let env = vec![(
        OsString::from("CAPSTAN_TRUST_DIR"),
        OsString::from("relative/dir"),
    )];
    let (exit, out, _) = bed.run(&["init"], env);
    assert_eq!(exit, 0, "{out}");
    assert!(
        out.contains("Wrote capstan.toml but did not trust it: CAPSTAN_TRUST_DIR relative/dir is not an absolute path"),
        "{out}"
    );
    assert!(
        out.contains(". Set CAPSTAN_TRUST_DIR to an absolute directory, then run cstan trust.\n"),
        "{out}"
    );
    assert!(bed.project.join("capstan.toml").is_file());
}

#[test]
fn a_local_override_file_is_not_part_of_the_record() {
    let bed = TrustBed::new("init-local");
    std::fs::write(
        bed.project.join("capstan.local.toml"),
        "[cache]\nmax_size_gb = 9\n",
    )
    .unwrap();
    let (exit, out, _) = bed.run(&["init"], bed.env());
    assert_eq!(exit, 0, "{out}");
    assert!(out.contains(TRUSTED), "{out}");
    // The record covers the checked keys of capstan.toml; the override file adds none of its own.
    assert_eq!(
        trust_status_with(&bed.project, &bed.store()),
        TrustStatus::Trusted
    );
    let pair = read_config_pair(&bed.project).unwrap().unwrap();
    let subset = checked_subset(&pair).unwrap();
    assert!(subset.values().all(|v| format!("{v}") != "9"), "{subset:?}");
    let main_only = std::fs::read(bed.project.join("capstan.toml")).unwrap();
    assert_eq!(pair.main, main_only);
}

#[test]
fn init_in_a_linked_worktree_is_trusted_for_the_main_checkout() {
    let bed = TrustBed::new("init-linked");
    let main = bed.temp.join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "--quiet"]);
    git(
        &main,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "chore: initial commit",
        ],
    );
    let linked = bed.temp.join("linked");
    git(
        &main,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
    );
    let outcome = {
        let bed_linked = TrustBed {
            temp: bed.temp.clone(),
            project: linked.clone(),
            store_dir: bed.store_dir.clone(),
        };
        let result = bed_linked.run(&["init"], bed_linked.env());
        std::mem::forget(bed_linked);
        result
    };
    assert_eq!(outcome.0, 0, "{}", outcome.1);
    assert!(outcome.1.contains(TRUSTED), "{}", outcome.1);
    let written = record_current(&linked, &bed.store()).unwrap();
    assert!(written.linked_worktree);
    // The main checkout holds the same file, so it reads as trusted too.
    std::fs::copy(linked.join("capstan.toml"), main.join("capstan.toml")).unwrap();
    assert_eq!(trust_status_with(&main, &bed.store()), TrustStatus::Trusted);
}

//! The trust store of the harness: every process it starts gets `CAPSTAN_TRUST_DIR=<scratch root>/trust`, and the
//! fixture helpers say which projects the store trusts.

use capstan_blackbox::*;
use capstan_config::{trust_status_with, TrustStatus, TrustStore};
use std::ffi::OsString;

const CONFIG: &str = "schema_version = 1\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\nlaunch_command = \"claude\"\n";

fn store_of(w: &World) -> TrustStore {
    TrustStore::at(w.root.join("trust"))
}

#[test]
fn trust_project_reads_trusted_and_an_untrusted_write_reads_needed() {
    let w = World::builder("trust-fixture").bare().build();
    let store = store_of(&w);
    write_config_untrusted(&w.project, CONFIG);
    assert!(
        matches!(
            trust_status_with(&w.project, &store),
            TrustStatus::Needed { .. }
        ),
        "{:?}",
        trust_status_with(&w.project, &store)
    );
    trust_project(&w.project);
    assert_eq!(trust_status_with(&w.project, &store), TrustStatus::Trusted);
}

#[test]
fn every_process_the_harness_starts_gets_the_trust_dir_in_the_scratch_root() {
    let mut w = World::builder("trust-env")
        .stopped()
        .env("CAPSTAN_TRUST_DIR", "/nonexistent/own-value")
        .build();
    let want = w.root.join("trust");
    let env: std::collections::BTreeMap<_, _> = w.environment().into_iter().collect();
    assert_eq!(env["CAPSTAN_TRUST_DIR"], want.to_string_lossy());
    assert!(want.starts_with(&w.root));
    // A command with its own value, a daemon and what it starts all see the harness's.
    let own = w.cstan_with(
        &w.project,
        None,
        &["ping"],
        &[("CAPSTAN_TRUST_DIR", "/nonexistent/extra")],
        None,
    );
    assert!(!own.has("/nonexistent"), "{}", own.text());
    w.start();
    let pid = w.daemon_pid().expect("a daemon pid");
    let seen = std::fs::read(format!("/proc/{pid}/environ")).expect("the daemon's environment");
    let line = format!("CAPSTAN_TRUST_DIR={}", want.display());
    assert!(
        seen.split(|b| *b == 0).any(|v| v == line.as_bytes()),
        "the daemon has {line}"
    );
    w.stop();
}

#[test]
fn the_binary_choice_is_cstan_bin_when_set() {
    assert_eq!(
        binary_choice(Some(OsString::from("/x/cstan"))),
        Some("/x/cstan".into())
    );
    assert_eq!(binary_choice(None), None);
}

//! The trust setup of the live harness, without Herdr: `write_config_trusted` makes the project trusted in a private
//! store, and the environment handed to every `cstan` names that store.
#[path = "common/live_env.rs"]
mod live_env;

use capstan_config::{trust_status_with, TrustStatus, TrustStore};
use live_env::{with_trust_dir, write_config_trusted};
use std::collections::HashMap;

/// `launch_command` is a checked key: a change of it needs trust.
const FIRST: &str = "schema_version = 1\n[hosts.claude]\nkind = \"claude\"\n\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\nlaunch_command = \"claude\"\n\n";

#[test]
fn a_written_config_is_trusted_again_after_a_change_and_the_environment_names_the_store() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut environment = HashMap::new();
    let trust_dir = with_trust_dir(&mut environment, root.path());
    assert_eq!(
        environment.get("CAPSTAN_TRUST_DIR").map(String::as_str),
        Some(trust_dir.to_str().unwrap())
    );
    assert!(!trust_dir.starts_with(&repo));
    let store = TrustStore::at(&trust_dir);

    write_config_trusted(&repo, FIRST, &trust_dir);
    assert_eq!(trust_status_with(&repo, &store), TrustStatus::Trusted);

    write_config_trusted(
        &repo,
        &FIRST.replace(
            "launch_command = \"claude\"",
            "launch_command = \"claude --x\"",
        ),
        &trust_dir,
    );
    assert_eq!(trust_status_with(&repo, &store), TrustStatus::Trusted);
}

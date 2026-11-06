//! The trust fixture helpers: one store, one root the helper trusts and one it leaves unread.

mod common;

use std::os::unix::fs::PermissionsExt;

use capstan_config::{fixture_store, trust_status_with, TrustStatus};

const CHECKED: &str = "schema_version = 1\n\n[verify]\ncommands = [\"echo ok\"]\n";

#[test]
fn trusted_project_reads_trusted_and_write_config_untrusted_reads_needed() {
    let temp = tempfile::tempdir().unwrap();
    // A trust store directory is private to its user.
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = fixture_store(temp.path());
    let trusted = tempfile::tempdir().unwrap();
    let untrusted = tempfile::tempdir().unwrap();

    common::write_config_untrusted(trusted.path(), CHECKED);
    common::trusted_project(trusted.path(), &store);
    common::write_config_untrusted(untrusted.path(), CHECKED);

    assert!(matches!(
        trust_status_with(trusted.path(), &store),
        TrustStatus::Trusted
    ));
    assert!(matches!(
        trust_status_with(untrusted.path(), &store),
        TrustStatus::Needed { .. }
    ));
}

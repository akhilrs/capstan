//! The trust fixtures of `support`: a trusted project reads Trusted, an untrusted one reads Needed.
mod support;

use capstan_config::{trust_status_with, TrustStatus, TrustStore};

#[test]
fn trusted_project_reads_trusted_and_untrusted_reads_needed() {
    let temp = capstan_testkit::short_temp_root()
        .join(format!("capstan-trust-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let store = TrustStore::at(temp.join("store"));
    let trusted = temp.join("trusted");
    let untrusted = temp.join("untrusted");
    support::trusted_project(&trusted, &store);
    support::write_config_untrusted(&untrusted, support::TRUST_FIXTURE_TOML);
    assert_eq!(trust_status_with(&trusted, &store), TrustStatus::Trusted);
    assert!(matches!(
        trust_status_with(&untrusted, &store),
        TrustStatus::Needed { .. }
    ));
    let _ = std::fs::remove_dir_all(&temp);
}

//! `gate_set` reaches a verify gate (leased slot or shared fallback) and never an agent.
use capstan_cache::{GateScope, Runtime};
use capstan_launcher::cache_env::cache_extras;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn runtime(project: &Path, cache_root: &Path, rust_options: &str) -> Runtime {
    let bin = project.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    std::fs::write(&cargo, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
    let toml = format!(
        "schema_version = 1\n[nexora]\ntrack = \"never\"\n[hosts.claude]\nkind = \"claude\"\n[roles.pm]\nkind = \"PM\"\nhost = \"claude\"\n[cache]\nroot = \"{}\"\npresets = [\"rust\"]\n[cache.rust]\nmode = \"shared-build-dir\"\ntarget_dir = true\n{rust_options}",
        cache_root.display()
    );
    let config = capstan_config::parse_config(toml.as_bytes(), project).unwrap();
    Runtime::from_config(
        config.cache.as_ref().unwrap(),
        project,
        "p1",
        &BTreeMap::new(),
        bin.to_str().unwrap(),
    )
}

fn debug_vars(rt: &Runtime, scope: GateScope) -> Vec<(String, String)> {
    cache_extras(rt, scope)
        .unwrap()
        .into_iter()
        .filter(|(n, _)| n.starts_with("CARGO_PROFILE_"))
        .collect()
}

#[test]
fn a_gate_and_the_shared_fallback_carry_the_debug_off_variables_an_agent_does_not() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let rt = runtime(dir.path(), cache.path(), "");
    let both = vec![
        ("CARGO_PROFILE_DEV_DEBUG".to_string(), "0".to_string()),
        ("CARGO_PROFILE_TEST_DEBUG".to_string(), "0".to_string()),
    ];
    assert!(debug_vars(&rt, GateScope::Agent).is_empty());
    assert_eq!(debug_vars(&rt, GateScope::Gate { slot: 0 }), both);
    assert_eq!(debug_vars(&rt, GateScope::Shared), both);
    assert!(debug_vars(&rt, GateScope::Bare).is_empty());
}

#[test]
fn gate_debug_true_leaves_the_gate_without_the_variables() {
    let (dir, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let rt = runtime(dir.path(), cache.path(), "gate_debug = true\n");
    for scope in [GateScope::Gate { slot: 0 }, GateScope::Shared] {
        assert!(debug_vars(&rt, scope).is_empty());
    }
}

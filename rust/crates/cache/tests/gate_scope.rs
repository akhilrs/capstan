mod common;
use std::fs;
use std::path::Path;

use capstan_cache::{GateScope, Runtime};
use capstan_config::{CacheConfig, CacheEntry, CacheOrigin};
use common::*;

fn rust_entry(shared_build: bool) -> CacheEntry {
    let mut e = custom("rust", "SCCACHE_DIR", "sccache");
    e.origin = CacheOrigin::Preset;
    e.dirs
        .push(("CARGO_TARGET_DIR".into(), "cargo-target".into()));
    e.gate_dirs = vec![("CARGO_TARGET_DIR".into(), "gate-target".into())];
    if shared_build {
        e.dirs
            .push(("CARGO_BUILD_BUILD_DIR".into(), "cargo-build".into()));
        e.gate_dirs
            .push(("CARGO_BUILD_BUILD_DIR".into(), "gate-build".into()));
    }
    e.set = vec![("RUSTC_WRAPPER".into(), "sccache".into())];
    e
}

fn runtime_for(cfg: &CacheConfig, project: &Path, id: &str) -> Runtime {
    Runtime::from_config(
        cfg,
        project,
        id,
        &env(&[("HOME", "/nonexistent")]),
        "/usr/bin:/bin",
    )
}

fn get<'a>(list: &'a [(String, String)], name: &str) -> Option<&'a str> {
    list.iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

#[test]
fn agent_and_gate_differ_in_the_target_dir_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![rust_entry(false)];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    let root = rt.root.join("rust");
    let agent = rt.ensure_scoped(GateScope::Agent).unwrap();
    assert_eq!(agent, rt.ensure().unwrap());
    assert_eq!(
        get(&agent, "CARGO_TARGET_DIR"),
        Some(root.join("cargo-target").to_str().unwrap())
    );
    let g1 = rt.ensure_scoped(GateScope::Gate { slot: 1 }).unwrap();
    let g2 = rt.ensure_scoped(GateScope::Gate { slot: 2 }).unwrap();
    assert_eq!(
        get(&g1, "CARGO_TARGET_DIR"),
        Some(root.join("gate-target/p1/slot-1").to_str().unwrap())
    );
    assert_ne!(get(&g1, "CARGO_TARGET_DIR"), get(&g2, "CARGO_TARGET_DIR"));
    assert!(root.join("gate-target/p1/slot-1").is_dir());
    assert!(root.join("gate-target/p1/slot-2").is_dir());
    assert_eq!(get(&g1, "SCCACHE_DIR"), get(&agent, "SCCACHE_DIR"));
    assert_eq!(get(&g1, "RUSTC_WRAPPER"), Some("sccache"));
    assert_eq!(g1.len(), agent.len());
}

#[test]
fn shared_build_dir_mode_gates_the_build_dir_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![rust_entry(true)];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    let root = rt.root.join("rust");
    let gate = rt.ensure_scoped(GateScope::Gate { slot: 1 }).unwrap();
    assert_eq!(
        get(&gate, "CARGO_BUILD_BUILD_DIR"),
        Some(root.join("gate-build/p1/slot-1").to_str().unwrap())
    );
    let shared = rt.ensure_scoped(GateScope::Shared).unwrap();
    assert!(get(&shared, "CARGO_TARGET_DIR").is_none());
    assert!(get(&shared, "CARGO_BUILD_BUILD_DIR").is_none());
}

#[test]
fn shared_drops_the_gate_variables_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![rust_entry(false)];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    let agent = rt.ensure().unwrap();
    let shared = rt.ensure_scoped(GateScope::Shared).unwrap();
    assert!(get(&shared, "CARGO_TARGET_DIR").is_none());
    assert_eq!(get(&shared, "SCCACHE_DIR"), get(&agent, "SCCACHE_DIR"));
    assert_eq!(shared.len(), agent.len() - 1);
}

#[test]
fn bare_is_empty_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    let mut cfg = config(&base);
    cfg.entries = vec![rust_entry(false)];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    let before = tree(&rt.root);
    assert!(rt.ensure_scoped(GateScope::Bare).unwrap().is_empty());
    assert_eq!(tree(&rt.root), before);
}

#[test]
fn without_gate_dirs_every_scope_equals_agent() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut plain_rust = rust_entry(false);
    plain_rust.gate_dirs.clear();
    cfg.entries = vec![plain_rust, custom("python", "PIP_CACHE_DIR", "pip")];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    let agent = rt.ensure_scoped(GateScope::Agent).unwrap();
    assert_eq!(
        rt.ensure_scoped(GateScope::Gate { slot: 3 }).unwrap(),
        agent
    );
    assert_eq!(rt.ensure_scoped(GateScope::Shared).unwrap(), agent);
    assert!(!rt.root.join("python").join("gate-target").exists());
}

#[test]
fn a_preset_without_gate_dirs_is_identical_for_agent_and_gate() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![rust_entry(false), custom("python", "PIP_CACHE_DIR", "pip")];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    let agent = rt.ensure_scoped(GateScope::Agent).unwrap();
    let gate = rt.ensure_scoped(GateScope::Gate { slot: 0 }).unwrap();
    assert_eq!(get(&gate, "PIP_CACHE_DIR"), get(&agent, "PIP_CACHE_DIR"));
}

#[test]
fn gate_dirs_alone_do_not_enable_an_entry() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut e = rust_entry(false);
    e.dirs.clear();
    e.set.clear();
    cfg.entries = vec![e];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    assert!(!rt.entry("rust").unwrap().enabled);
    assert!(rt
        .ensure_scoped(GateScope::Gate { slot: 1 })
        .unwrap()
        .is_empty());
    assert!(!rt.root.join("rust").exists());
    let _ = fs::metadata(&rt.root);
}

fn debug_off() -> Vec<(String, String)> {
    vec![
        ("CARGO_PROFILE_DEV_DEBUG".into(), "0".into()),
        ("CARGO_PROFILE_TEST_DEBUG".into(), "0".into()),
    ]
}

fn debug_vars(env: &[(String, String)]) -> Vec<&str> {
    env.iter()
        .filter(|(n, _)| n.starts_with("CARGO_PROFILE_"))
        .map(|(n, _)| n.as_str())
        .collect()
}

#[test]
fn gate_and_shared_scope_get_gate_set_after_set_and_agent_and_bare_do_not() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut e = rust_entry(false);
    e.gate_set = debug_off();
    cfg.entries = vec![e];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    let gate = rt.ensure_scoped(GateScope::Gate { slot: 1 }).unwrap();
    let shared = rt.ensure_scoped(GateScope::Shared).unwrap();
    for env in [&gate, &shared] {
        assert_eq!(get(env, "CARGO_PROFILE_DEV_DEBUG"), Some("0"));
        assert_eq!(get(env, "CARGO_PROFILE_TEST_DEBUG"), Some("0"));
        let set = env.iter().position(|(n, _)| n == "RUSTC_WRAPPER").unwrap();
        let debug = env
            .iter()
            .position(|(n, _)| n == "CARGO_PROFILE_DEV_DEBUG")
            .unwrap();
        assert!(set < debug, "gate_set comes after set: {env:?}");
    }
    assert!(debug_vars(&rt.ensure_scoped(GateScope::Agent).unwrap()).is_empty());
    assert!(rt.ensure_scoped(GateScope::Bare).unwrap().is_empty());
}

#[test]
fn an_entry_without_gate_set_is_unchanged_in_every_scope() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![rust_entry(false)];
    let rt = runtime_for(&cfg, dir.path(), "p1");
    for scope in [GateScope::Gate { slot: 1 }, GateScope::Shared] {
        assert!(debug_vars(&rt.ensure_scoped(scope).unwrap()).is_empty());
    }
}

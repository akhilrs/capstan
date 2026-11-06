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

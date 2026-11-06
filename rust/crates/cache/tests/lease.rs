mod common;
use std::fs;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use capstan_cache::lease::{in_use, self_test, slot_leases, EntryGuard, GateLease};
use capstan_cache::{LeaseError, Runtime};
use capstan_config::{CacheConfig, CacheEntry, CacheOrigin, CacheScope};
use common::*;

fn gated(name: &str, var: &str, sub: &str, gate_sub: &str) -> CacheEntry {
    let mut e = custom(name, var, sub);
    e.origin = CacheOrigin::Preset;
    e.gate_dirs = vec![(var.into(), gate_sub.into())];
    e
}

fn rt(cfg: &CacheConfig, project: &Path, id: &str) -> Runtime {
    Runtime::from_config(
        cfg,
        project,
        id,
        &env(&[("HOME", "/nonexistent")]),
        "/usr/bin:/bin",
    )
}

fn one(base: &Path) -> CacheConfig {
    let mut cfg = config(base);
    cfg.entries = vec![gated(
        "rust",
        "CARGO_TARGET_DIR",
        "cargo-target",
        "gate-target",
    )];
    cfg
}

const WAIT: Duration = Duration::from_millis(300);

fn no_abort() -> AtomicBool {
    AtomicBool::new(false)
}

fn poll(mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn entry_dir(rt: &Runtime) -> PathBuf {
    rt.entry_dir("rust")
}

#[test]
fn held_is_in_use_dropped_is_free_and_probes_create_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rt(&one(&dir.path().join("b")), dir.path(), "p1");
    let ed = entry_dir(&runtime);
    let before = tree(&runtime.root);
    assert!(!in_use(&ed, "p1", 0));
    assert!(GateLease::try_acquire(&ed, "p1", 0).is_none());
    assert_eq!(tree(&runtime.root), before, "probes create nothing");

    let abort = no_abort();
    let leases = runtime.lease_gate(0, WAIT, &abort).unwrap();
    assert!(!leases.is_empty());
    assert!(in_use(&ed, "p1", 0));
    assert!(GateLease::try_acquire(&ed, "p1", 0).is_none());
    drop(leases);
    assert!(!in_use(&ed, "p1", 0));
    let again = GateLease::try_acquire(&ed, "p1", 0).expect("free after drop");
    assert!(in_use(&ed, "p1", 0));
    drop(again);
}

#[test]
fn a_stale_lease_file_is_free() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rt(&one(&dir.path().join("b")), dir.path(), "p1");
    let ed = entry_dir(&runtime);
    fs::create_dir_all(&ed).unwrap();
    let file = ed.join(".slot-p1-4.lease");
    fs::write(&file, "").unwrap();
    fs::File::options()
        .append(true)
        .open(&file)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(86_400))
        .unwrap();
    assert!(!in_use(&ed, "p1", 4));
    assert!(GateLease::try_acquire(&ed, "p1", 4).is_some());
}

#[test]
fn a_child_does_not_inherit_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rt(&one(&dir.path().join("b")), dir.path(), "p1");
    let ed = entry_dir(&runtime);
    let abort = no_abort();
    let leases = runtime.lease_gate(0, WAIT, &abort).unwrap();
    let child = Kill(Command::new("sleep").arg("30").spawn().unwrap());
    drop(leases);
    assert!(
        poll(|| GateLease::try_acquire(&ed, "p1", 0).is_some()),
        "a live child must not keep the slot busy"
    );
    drop(child);
}

#[test]
fn the_lease_descriptor_is_close_on_exec() {
    let dir = tempfile::tempdir().unwrap();
    let ed = dir.path().join("rust");
    fs::create_dir_all(&ed).unwrap();
    let lease = GateLease::acquire(&ed, "p1", 0).unwrap();
    // SAFETY: the descriptor is open for as long as `lease` lives.
    let flags = unsafe { libc::fcntl(lease.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
}

#[test]
fn removing_the_slot_dir_while_held_leaves_the_lease_held() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rt(&one(&dir.path().join("b")), dir.path(), "p1");
    let ed = entry_dir(&runtime);
    let abort = no_abort();
    let leases = runtime.lease_gate(2, WAIT, &abort).unwrap();
    let slot = ed.join("gate-target/p1/slot-2");
    assert!(slot.is_dir());
    fs::remove_dir_all(&slot).unwrap();
    assert!(in_use(&ed, "p1", 2));
    drop(leases);
}

#[test]
fn a_renamed_lease_file_is_retried_and_the_new_file_locked() {
    let dir = tempfile::tempdir().unwrap();
    let ed = dir.path().join("rust");
    fs::create_dir_all(&ed).unwrap();
    let path = ed.join(".slot-p1-0.lease");
    fs::write(&path, "").unwrap();
    let moved = std::cell::Cell::new(false);
    let lease = GateLease::acquire_hooked(&ed, "p1", 0, &|| {
        if !moved.replace(true) {
            fs::rename(&path, ed.join("renamed")).unwrap();
        }
    })
    .unwrap();
    assert!(path.exists(), "a new file was created");
    assert!(in_use(&ed, "p1", 0));
    drop(lease);
}

#[test]
fn a_renamed_guard_file_is_retried_too() {
    let dir = tempfile::tempdir().unwrap();
    let ed = dir.path().join("rust");
    fs::create_dir_all(&ed).unwrap();
    let path = ed.join(".capstan-gate.lock");
    fs::write(&path, "").unwrap();
    let moved = std::cell::Cell::new(false);
    let guard = EntryGuard::acquire_hooked(&ed, true, &|| {
        if !moved.replace(true) {
            fs::rename(&path, ed.join("trashed")).unwrap();
        }
    })
    .unwrap();
    assert!(path.exists());
    assert_eq!(EntryGuard::try_shared(&ed).err(), Some(LeaseError::Busy));
    drop(guard);
    assert!(EntryGuard::try_shared(&ed).is_ok());
}

#[test]
fn busy_after_the_wait_while_the_guard_is_held_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rt(&one(&dir.path().join("b")), dir.path(), "p1");
    let ed = entry_dir(&runtime);
    fs::create_dir_all(&ed).unwrap();
    let _guard = EntryGuard::try_exclusive(&ed).unwrap();
    let start = Instant::now();
    let abort = no_abort();
    let result = runtime.lease_gate(0, Duration::from_millis(200), &abort);
    assert_eq!(result.err(), Some(LeaseError::Busy));
    assert!(start.elapsed() >= Duration::from_millis(150));
}

#[test]
fn abort_ends_a_long_wait_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rt(&one(&dir.path().join("b")), dir.path(), "p1");
    let ed = entry_dir(&runtime);
    fs::create_dir_all(&ed).unwrap();
    let _guard = EntryGuard::try_exclusive(&ed).unwrap();
    let abort = Arc::new(AtomicBool::new(false));
    let flag = abort.clone();
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, Ordering::SeqCst);
    });
    let start = Instant::now();
    let result = runtime.lease_gate(0, Duration::from_secs(10), &abort);
    setter.join().unwrap();
    assert_eq!(result.err(), Some(LeaseError::Aborted));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn lease_gate_is_all_or_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("b"));
    cfg.entries = vec![
        gated("a", "A_DIR", "a", "ga"),
        gated("b", "B_DIR", "b", "gb"),
    ];
    let runtime = rt(&cfg, dir.path(), "p1");
    let a = runtime.entry_dir("a");
    let b = runtime.entry_dir("b");
    fs::create_dir_all(&b).unwrap();
    let _held = GateLease::acquire(&b, "p1", 0).unwrap();
    let abort = no_abort();
    let result = runtime.lease_gate(0, Duration::from_millis(100), &abort);
    assert_eq!(result.err(), Some(LeaseError::Busy));
    assert!(
        GateLease::try_acquire(&a, "p1", 0).is_some(),
        "A was released"
    );
}

#[test]
fn reacquire_recreates_removed_dirs_and_holds_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = rt(&one(&dir.path().join("b")), dir.path(), "p1");
    let ed = entry_dir(&runtime);
    let abort = no_abort();
    let mut leases = runtime.lease_gate(1, WAIT, &abort).unwrap();
    let slot = ed.join("gate-target/p1/slot-1");
    fs::remove_dir_all(ed.join("gate-target")).unwrap();
    leases.reacquire(WAIT, &abort).unwrap();
    assert!(slot.is_dir());
    assert!(in_use(&ed, "p1", 1));
}

#[test]
fn lease_gate_without_gate_dirs_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("b"));
    cfg.entries = vec![custom("python", "PIP_CACHE_DIR", "pip")];
    let runtime = rt(&cfg, dir.path(), "p1");
    let before = tree(&runtime.root);
    let abort = no_abort();
    let leases = runtime.lease_gate(0, WAIT, &abort).unwrap();
    assert!(leases.is_empty());
    assert_eq!(tree(&runtime.root), before);
}

#[test]
fn self_test_passes_on_a_local_dir_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    assert!(self_test(dir.path()));
    assert!(tree(dir.path()).is_empty());
    assert!(!self_test(&dir.path().join("missing")));
}

#[test]
fn slot_leases_parse_project_ids_with_hyphens() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        ".slot-6f1c-aa-b2-3.lease",
        ".slot-x.lease",
        ".slot-p-abc.lease",
        "other",
    ] {
        fs::write(dir.path().join(name), "").unwrap();
    }
    let found = slot_leases(dir.path());
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].0.as_str(), found[0].1), ("6f1c-aa-b2", 3));
}

#[test]
fn two_projects_on_a_user_root_lease_the_same_slot_apart() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = one(&dir.path().join("b"));
    cfg.scope = CacheScope::User;
    let r1 = rt(&cfg, dir.path(), "p1");
    let r2 = rt(&cfg, dir.path(), "p2");
    assert_eq!(r1.root, r2.root);
    let abort = no_abort();
    let l1 = r1.lease_gate(0, WAIT, &abort).unwrap();
    let l2 = r2.lease_gate(0, WAIT, &abort).unwrap();
    let s1 = r1.gate_slots();
    let s2 = r2.gate_slots();
    assert_eq!(s1.len(), 1);
    assert_eq!(s2.len(), 1);
    assert!(s1[0].2[0].to_string_lossy().contains("/p1/"));
    assert!(s2[0].2[0].to_string_lossy().contains("/p2/"));
    // A held slot is not removed, and one project never removes the other's.
    assert!(!r1.remove_slot("rust", 0).unwrap());
    assert!(s1[0].2[0].exists());
    drop(l1);
    assert!(r1.remove_slot("rust", 0).unwrap());
    assert!(!s1[0].2[0].exists());
    assert!(s2[0].2[0].exists());
    assert!(in_use(&r2.entry_dir("rust"), "p2", 0));
    assert_eq!(r2.gate_slots().len(), 1);
    assert!(r1.gate_slots().is_empty());
    drop(l2);
}

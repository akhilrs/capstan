//! Lease tests that fork children. They live in their own binary: a child forked by one test holds a copy of every
//! descriptor open in the process until its exec, so a flock can outlive its drop by a few milliseconds. Next to the
//! other lease tests that would make their `!in_use` checks after a drop flaky under load.
mod common;
use std::fs;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use capstan_cache::lease::{in_use, GateLease};
use capstan_config::{CacheConfig, CacheEntry, CacheOrigin};
use common::*;

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

/// True once the slot is free. A lease descriptor is close-on-exec, but a child that another test in this binary
/// forks holds a copy of it until its exec, so the flock can outlive the drop by a few milliseconds under load.
fn free_soon(ed: &Path, project: &str, slot: usize) -> bool {
    poll(|| !in_use(ed, project, slot))
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn gated_entry(base: &Path) -> std::path::PathBuf {
    let mut cfg: CacheConfig = config(base);
    let mut e: CacheEntry = custom("rust", "CARGO_TARGET_DIR", "cargo-target");
    e.origin = CacheOrigin::Preset;
    e.gate_dirs = vec![("CARGO_TARGET_DIR".into(), "gate-target".into())];
    cfg.entries = vec![e];
    let runtime = capstan_cache::Runtime::from_config(
        &cfg,
        base,
        "p1",
        &env(&[("HOME", "/nonexistent")]),
        "/usr/bin:/bin",
    );
    runtime.entry_dir("rust")
}

#[test]
fn a_child_does_not_inherit_the_lease() {
    let dir = tempfile::tempdir().unwrap();
    let ed = gated_entry(&dir.path().join("b"));
    fs::create_dir_all(&ed).unwrap();
    let leases = GateLease::acquire(&ed, "p1", 0).unwrap();
    let child = Kill(Command::new("sleep").arg("30").spawn().unwrap());
    drop(leases);
    assert!(
        poll(|| GateLease::try_acquire(&ed, "p1", 0).is_some()),
        "a live child must not keep the slot busy"
    );
    drop(child);
}

/// Regression for the fork-to-exec window: a thread keeps spawning children while leases are taken and dropped.
#[test]
fn a_dropped_lease_is_free_soon_while_children_are_spawned() {
    let dir = tempfile::tempdir().unwrap();
    let ed = dir.path().join("rust");
    fs::create_dir_all(&ed).unwrap();
    // `try_acquire` never creates the lease file, so create it before any child is forked.
    drop(GateLease::acquire(&ed, "p1", 0).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let spawners: Vec<_> = (0..4)
        .map(|_| {
            let flag = flag.clone();
            std::thread::spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    let _ = Command::new("true").status();
                }
            })
        })
        .collect();
    for i in 0..200 {
        // The probe's own descriptor can sit in a forked child for a moment too, so the take is polled as well.
        let mut taken = None;
        assert!(
            poll(|| {
                taken = GateLease::try_acquire(&ed, "p1", 0);
                taken.is_some()
            }),
            "take {i}"
        );
        let lease = taken.unwrap();
        assert!(in_use(&ed, "p1", 0));
        drop(lease);
        let free = free_soon(&ed, "p1", 0);
        if !free {
            stop.store(true, Ordering::SeqCst);
        }
        assert!(free, "iteration {i}");
        std::thread::sleep(Duration::from_millis(1));
    }
    stop.store(true, Ordering::SeqCst);
    for s in spawners {
        s.join().unwrap();
    }
}

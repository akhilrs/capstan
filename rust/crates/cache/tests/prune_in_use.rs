//! Prune and clear never remove a gate slot whose lease is held, and keep the entry guard only while they rename.
mod common;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use capstan_cache::lease::{slot_label, EntryGuard, GateLease, IN_USE_REASON};
use capstan_cache::{
    clear_with_hooks, prune, prune_with_hooks, read_manifest, CacheError, GateLeases, Policy,
    PruneHooks, PruneReport, Runtime, GUARD_BUSY_REASON,
};
use capstan_config::{CacheConfig, CacheEntry, CacheOrigin, CacheScope};
use common::*;

const MB: u64 = 1_000_000;
const DAY: u64 = 86_400;

fn rust_entry() -> CacheEntry {
    let mut e = custom("rust", "SCCACHE_DIR", "sccache");
    e.origin = CacheOrigin::Preset;
    e.dirs
        .push(("CARGO_TARGET_DIR".into(), "cargo-target".into()));
    e.gate_dirs = vec![("CARGO_TARGET_DIR".into(), "gate-target".into())];
    e
}

struct World {
    _dir: tempfile::TempDir,
    rt: Runtime,
}

fn world_with(scope: CacheScope) -> World {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg: CacheConfig = config(&dir.path().join("base"));
    cfg.scope = scope;
    cfg.entries = vec![rust_entry()];
    let project = dir.path().join("proj");
    repo(&project);
    let rt = runtime(&cfg, &project);
    rt.ensure().unwrap();
    World { _dir: dir, rt }
}

fn world() -> World {
    world_with(CacheScope::Project)
}

fn entry_dir(w: &World) -> PathBuf {
    w.rt.entry_dir("rust")
}

fn slot_dir(w: &World, project: &str, slot: usize) -> PathBuf {
    entry_dir(w)
        .join("gate-target")
        .join(project)
        .join(format!("slot-{slot}"))
}

/// A slot of `bytes` whose lease file was last used `age_days` ago; nobody holds it.
fn make_slot(w: &World, project: &str, slot: usize, bytes: u64, age_days: u64) {
    sparse(&slot_dir(w, project, slot).join("blob"), bytes);
    let lease = entry_dir(w).join(format!(".slot-{project}-{slot}.lease"));
    fs::write(&lease, "").unwrap();
    age(&lease, age_days * DAY);
}

fn hold(w: &World, project: &str, slot: usize) -> GateLease {
    GateLease::acquire(&entry_dir(w), project, slot).unwrap()
}

fn policy(max_mb: u64, idle: bool) -> Policy {
    Policy {
        max_bytes: max_mb * MB,
        max_age_ms: 3650 * DAY * 1000,
        idle,
    }
}

/// A lock descriptor can outlive its holder for an instant in a child another test thread forked: `Busy` then is
/// transient.
fn retry<T>(mut call: impl FnMut() -> Result<T, CacheError>) -> T {
    for _ in 0..500 {
        match call() {
            Err(CacheError::Busy) => std::thread::sleep(Duration::from_millis(10)),
            other => return other.unwrap(),
        }
    }
    call().unwrap()
}

/// Poll `free` for up to 5 s. A child another test thread forked keeps a copy of a just-dropped lock descriptor
/// until its exec, so a lock can look held for a few ms after its owner dropped it. Strict "still held" checks
/// stay strict: only the moment right after a drop waits.
fn wait_until(what: &str, mut free: impl FnMut() -> bool) {
    for _ in 0..500 {
        if free() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} still held 5 s after its drop");
}

fn wait_slot_free(w: &World, project: &str, slot: usize) {
    wait_until("the slot lease", || {
        !capstan_cache::lease::in_use(&entry_dir(w), project, slot)
    });
}

fn wait_guard_free(w: &World) {
    wait_until("the entry guard", || {
        EntryGuard::try_exclusive(&entry_dir(w)).is_ok()
    });
}

/// Retries `call` while it skips an "in use" slot that no test holds: the prune's own probe descriptor can be
/// copied by a sibling's fork and look held for a few ms. A skip only happens before anything was changed there,
/// so a retry is safe. The `held` slots of project `proj` must stay skipped, so those checks stay strict; a slot
/// that is wrongly held still shows after 5 s and fails the test's own assertions.
fn settle(
    held: &[usize],
    mut call: impl FnMut() -> Result<PruneReport, CacheError>,
) -> PruneReport {
    let expected: Vec<String> = held.iter().map(|s| label(*s)).collect();
    for _ in 0..500 {
        let report = retry(&mut call);
        if skipped_labels(&report, IN_USE_REASON)
            .iter()
            .all(|l| expected.contains(l))
        {
            return report;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    retry(call)
}

fn run(w: &World, held: &[usize], p: Policy) -> PruneReport {
    settle(held, || prune(&w.rt, p, &env(&[]), &ok_runner))
}

fn label(slot: usize) -> String {
    slot_label("rust", "proj", slot)
}

fn skipped_labels(report: &PruneReport, reason: &str) -> Vec<String> {
    report
        .skipped
        .iter()
        .filter(|s| s.reason == reason)
        .map(|s| s.label.clone())
        .collect()
}

#[test]
fn a_held_lease_keeps_its_slot_and_a_released_one_goes_next_time() {
    let w = world();
    make_slot(&w, "proj", 0, 100 * MB, 1);
    sparse(&entry_dir(&w).join("cargo-target/blob"), 100 * MB);
    let held = hold(&w, "proj", 0);

    let dry = run(&w, &[0], policy(150, false));
    assert!(dry.would_clear.is_empty(), "{dry:?}");
    assert_eq!(skipped_labels(&dry, IN_USE_REASON), [label(0)]);
    let report = run(&w, &[0], policy(150, true));
    assert!(report.cleared.is_empty(), "{report:?}");
    assert_eq!(skipped_labels(&report, IN_USE_REASON), [label(0)]);
    assert!(slot_dir(&w, "proj", 0).join("blob").exists());
    assert!(
        report.total_after > 150 * MB,
        "the cap is not reached and the report says so"
    );

    drop(held);
    wait_slot_free(&w, "proj", 0);
    let report = run(&w, &[], policy(150, true));
    assert_eq!(report.cleared, [label(0)], "{report:?}");
    assert!(!slot_dir(&w, "proj", 0).exists());
    assert!(!entry_dir(&w).join(".slot-proj-0.lease").exists());
    assert!(entry_dir(&w).join("cargo-target/blob").exists());
    let manifest = read_manifest(&w.rt.root).unwrap().unwrap();
    let rust = manifest.entries.iter().find(|e| e.name == "rust").unwrap();
    assert!(rust.bytes <= 150 * MB && rust.bytes >= 100 * MB, "{rust:?}");
}

#[test]
fn an_unleased_slot_goes_before_the_shared_caches() {
    let w = world();
    sparse(&entry_dir(&w).join("sccache/blob"), 100 * MB);
    sparse(&entry_dir(&w).join("cargo-target/blob"), 100 * MB);
    make_slot(&w, "proj", 0, 100 * MB, 1);
    let report = run(&w, &[], policy(250, true));
    assert_eq!(report.cleared, [label(0)], "{report:?}");
    assert!(entry_dir(&w).join("sccache/blob").exists());
    assert!(entry_dir(&w).join("cargo-target/blob").exists());
    assert!(report.total_after <= 250 * MB);
}

#[test]
fn the_cap_evicts_unleased_slots_oldest_lease_file_first() {
    let w = world();
    make_slot(&w, "proj", 0, 100 * MB, 9);
    make_slot(&w, "proj", 1, 100 * MB, 3);
    make_slot(&w, "proj", 2, 100 * MB, 8);
    make_slot(&w, "other", 3, 100 * MB, 2);
    let (a, b) = (hold(&w, "proj", 0), hold(&w, "proj", 2));
    let report = run(&w, &[0, 2], policy(300, true));
    assert_eq!(
        report.cleared,
        [slot_label("rust", "proj", 1)],
        "{report:?}"
    );
    assert!(slot_dir(&w, "other", 3).exists());
    let report = run(&w, &[0, 2], policy(150, true));
    assert_eq!(report.cleared, [slot_label("rust", "other", 3)]);
    assert!(slot_dir(&w, "proj", 0).exists() && slot_dir(&w, "proj", 2).exists());
    let mut leased = skipped_labels(&report, IN_USE_REASON);
    leased.sort();
    assert_eq!(leased, [label(0), label(2)]);
    assert!(report.total_after > 150 * MB, "nothing is hidden");
    drop((a, b));
}

#[test]
fn age_removes_only_unleased_expired_slots() {
    let w = world();
    make_slot(&w, "proj", 0, MB, 40);
    make_slot(&w, "proj", 1, MB, 40);
    make_slot(&w, "proj", 2, MB, 40);
    make_slot(&w, "proj", 3, MB, 5);
    let (a, b) = (hold(&w, "proj", 1), hold(&w, "proj", 2));
    let p = Policy {
        max_age_ms: 10 * DAY * 1000,
        ..policy(1000, true)
    };
    let report = settle(&[1, 2], || prune(&w.rt, p, &env(&[]), &ok_runner));
    assert_eq!(report.cleared, [label(0)], "{report:?}");
    assert!(slot_dir(&w, "proj", 3).exists());
    assert!(slot_dir(&w, "proj", 1).exists() && slot_dir(&w, "proj", 2).exists());
    drop((a, b));
}

#[test]
fn an_expired_entry_with_a_leased_slot_loses_only_its_unleased_slots() {
    let w = world();
    sparse(&entry_dir(&w).join("cargo-target/blob"), MB);
    age(&entry_dir(&w).join(".last-used"), 40 * DAY);
    make_slot(&w, "proj", 0, MB, 0);
    make_slot(&w, "proj", 1, MB, 0);
    let held = hold(&w, "proj", 1);
    let p = Policy {
        max_age_ms: 10 * DAY * 1000,
        ..policy(1000, true)
    };
    let report = settle(&[1], || prune(&w.rt, p, &env(&[]), &ok_runner));
    assert_eq!(report.cleared, [label(0)], "{report:?}");
    assert_eq!(skipped_labels(&report, IN_USE_REASON), [label(1)]);
    assert!(entry_dir(&w).join("cargo-target/blob").exists());
    drop(held);
}

#[test]
fn clear_keeps_a_leased_slot_with_force_and_a_slot_of_another_project() {
    for (scope, force) in [(CacheScope::Project, true), (CacheScope::User, false)] {
        let w = world_with(scope);
        make_slot(&w, "proj", 0, MB, 1);
        make_slot(&w, "other", 1, MB, 1);
        make_slot(&w, "other", 2, MB, 1);
        let held = hold(&w, "other", 1);
        let report = retry(|| capstan_cache::clear(&w.rt, None, force, &env(&[]), &ok_runner));
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(slot_dir(&w, "other", 1).exists(), "leased slot stays");
        assert!(!slot_dir(&w, "other", 2).exists());
        assert!(!slot_dir(&w, "proj", 0).exists());
        assert!(entry_dir(&w).is_dir());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].reason, IN_USE_REASON);
        assert!(report
            .skipped
            .iter()
            .any(|s| s.label == slot_label("rust", "other", 1)));
        drop(held);
        wait_slot_free(&w, "other", 1);
        let report = retry(|| capstan_cache::clear(&w.rt, None, force, &env(&[]), &ok_runner));
        assert!(report.cleared.contains(&"rust".to_string()), "{report:?}");
        assert!(!entry_dir(&w).exists());
    }
}

fn no_abort() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn the_guard_is_held_across_the_rename_and_a_gate_waits_then_uses_the_fresh_entry() {
    let w = world();
    make_slot(&w, "proj", 0, MB, 1);
    let abort = no_abort();
    let seen = Mutex::new(None);
    std::thread::scope(|scope| {
        let gate = RefCell::new(None);
        let hook = |dir: &Path| {
            assert_eq!(dir, entry_dir(&w));
            let handle = scope.spawn(|| {
                w.rt.lease_gate(7, Duration::from_secs(10), &abort)
                    .map_err(|e| e.to_string())
            });
            std::thread::sleep(Duration::from_millis(300));
            *seen.lock().unwrap() = Some(slot_dir(&w, "proj", 7).exists());
            *gate.borrow_mut() = Some(handle);
        };
        let hooks = PruneHooks {
            before_rename: &hook,
            after_rename: &|| {},
        };
        let report = retry(|| clear_with_hooks(&w.rt, None, true, &env(&[]), &ok_runner, &hooks));
        assert_eq!(report.cleared, ["rust"]);
        let leases = gate.borrow_mut().take().unwrap().join().unwrap().unwrap();
        assert_eq!(*seen.lock().unwrap(), Some(false), "the gate waited");
        assert!(slot_dir(&w, "proj", 7).is_dir(), "in the fresh entry");
        assert!(!leases.is_empty());
    });
}

#[test]
fn a_slot_eviction_holds_the_guard_too() {
    let w = world();
    make_slot(&w, "proj", 0, 100 * MB, 1);
    let abort = no_abort();
    std::thread::scope(|scope| {
        let gate = RefCell::new(None);
        let seen = RefCell::new(true);
        let hook = |_: &Path| {
            let handle = scope.spawn(|| w.rt.lease_gate(7, Duration::from_secs(10), &abort));
            std::thread::sleep(Duration::from_millis(300));
            *seen.borrow_mut() = slot_dir(&w, "proj", 7).exists();
            *gate.borrow_mut() = Some(handle);
        };
        let hooks = PruneHooks {
            before_rename: &hook,
            after_rename: &|| {},
        };
        let report =
            retry(|| prune_with_hooks(&w.rt, policy(10, true), &env(&[]), &ok_runner, &hooks));
        assert_eq!(report.cleared, [label(0)], "{report:?}");
        let leases: GateLeases = gate.borrow_mut().take().unwrap().join().unwrap().unwrap();
        assert!(!*seen.borrow());
        assert!(slot_dir(&w, "proj", 7).is_dir());
        drop(leases);
    });
}

#[test]
fn an_acquire_between_rename_and_trash_removal_gets_a_slot_outside_the_trash() {
    let w = world();
    make_slot(&w, "proj", 0, MB, 1);
    let abort = no_abort();
    let got: RefCell<Option<GateLeases>> = RefCell::new(None);
    let after = || {
        let leases =
            w.rt.lease_gate(3, Duration::from_secs(5), &abort)
                .expect("the guard is free once the entry is renamed");
        *got.borrow_mut() = Some(leases);
    };
    let hooks = PruneHooks {
        before_rename: &|_| {},
        after_rename: &after,
    };
    let report = retry(|| clear_with_hooks(&w.rt, None, true, &env(&[]), &ok_runner, &hooks));
    assert_eq!(report.cleared, ["rust"]);
    assert!(slot_dir(&w, "proj", 3).is_dir());
    assert!(
        !tree(&w.rt.root.join(".trash"))
            .iter()
            .any(|p| p.contains("slot-3")),
        "not in the trash"
    );
    drop(got);
}

#[test]
fn a_stop_command_runs_while_the_guard_is_free() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut entry = rust_entry();
    entry.stop = vec!["sleep 2".into()];
    cfg.entries = vec![entry];
    let project = dir.path().join("proj");
    repo(&project);
    let rt = runtime(&cfg, &project);
    rt.ensure().unwrap();
    let finished = AtomicBool::new(false);
    let abort = no_abort();
    std::thread::scope(|scope| {
        let clearing = scope.spawn(|| {
            let report = retry(|| capstan_cache::clear(&rt, None, true, &env(&[]), &shell_runner));
            finished.store(true, Ordering::SeqCst);
            report
        });
        let end = Instant::now() + Duration::from_secs(10);
        let mut got = None;
        while got.is_none() && Instant::now() < end {
            match rt.lease_gate(4, Duration::ZERO, &abort) {
                Ok(leases) => got = Some(leases),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        assert!(got.is_some(), "a gate acquired meanwhile");
        assert!(
            !finished.load(Ordering::SeqCst),
            "while the stop command was still running"
        );
        let report = clearing.join().unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(report.stop_commands.len(), 1);
    });
}

#[test]
fn a_slot_leased_before_the_prune_is_listed_and_kept_until_released() {
    let w = world();
    make_slot(&w, "proj", 0, 100 * MB, 1);
    let held = hold(&w, "proj", 0);
    let renames = RefCell::new(0);
    let before = |_: &Path| *renames.borrow_mut() += 1;
    let hooks = PruneHooks {
        before_rename: &before,
        after_rename: &|| {},
    };
    let report = settle(&[0], || {
        prune_with_hooks(&w.rt, policy(10, true), &env(&[]), &ok_runner, &hooks)
    });
    assert_eq!(skipped_labels(&report, IN_USE_REASON), [label(0)]);
    assert!(slot_dir(&w, "proj", 0).exists());
    assert_eq!(*renames.borrow(), 0, "nothing was renamed");
    drop(held);
    wait_slot_free(&w, "proj", 0);
    let report = settle(&[], || {
        prune_with_hooks(&w.rt, policy(10, true), &env(&[]), &ok_runner, &hooks)
    });
    assert_eq!(report.cleared, [label(0)]);
    assert_eq!(*renames.borrow(), 1);
}

#[test]
fn a_busy_guard_skips_the_entry_with_its_own_reason() {
    let w = world();
    make_slot(&w, "proj", 0, 100 * MB, 1);
    let guard = EntryGuard::try_shared(&entry_dir(&w)).unwrap();
    let report = run(&w, &[], policy(10, true));
    assert!(report.cleared.is_empty(), "{report:?}");
    assert_eq!(skipped_labels(&report, GUARD_BUSY_REASON), ["rust"]);
    assert!(skipped_labels(&report, IN_USE_REASON).is_empty());
    assert!(slot_dir(&w, "proj", 0).exists());
    assert!(report.total_after > 10 * MB);
    drop(guard);
    wait_guard_free(&w);
    let report = run(&w, &[], policy(10, true));
    assert_eq!(report.cleared[0], label(0), "{report:?}");
}

#[test]
fn a_lease_is_never_pruned_under_a_holder() {
    let w = world();
    sparse(&entry_dir(&w).join("cargo-target/blob"), MB);
    let abort = no_abort();
    let state: Mutex<Option<GateLeases>> = Mutex::new(None);
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    // A failed assertion must still stop the holder, or the scope never ends.
    struct Stop<'a>(&'a AtomicBool);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    std::thread::scope(|scope| {
        let _stop = Stop(&stop);
        let holder = scope.spawn(|| {
            while !stop.load(Ordering::SeqCst) {
                let leases =
                    w.rt.lease_gate(0, Duration::from_secs(30), &abort)
                        .expect("lease");
                *state.lock().unwrap() = Some(leases);
                std::thread::sleep(Duration::from_millis(2));
                *state.lock().unwrap() = None;
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        for round in 0..200 {
            // A lock descriptor can outlive its holder for an instant in a child another test thread forked.
            let mut result = prune(&w.rt, policy(0, true), &env(&[]), &ok_runner);
            for _ in 0..500 {
                if !matches!(result, Err(CacheError::Busy)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
                result = prune(&w.rt, policy(0, true), &env(&[]), &ok_runner);
            }
            let report = result.unwrap_or_else(|e| panic!("round {round}: {e}"));
            assert!(report.errors.is_empty(), "round {round}: {report:?}");
            let held = state.lock().unwrap();
            if held.is_some() {
                assert!(
                    slot_dir(&w, "proj", 0).is_dir() || slot_dir(&w, &segment_of(&w), 0).is_dir(),
                    "round {round}: a held slot lost its directories"
                );
            }
            assert!(started.elapsed() < Duration::from_secs(600), "hang");
        }
        stop.store(true, Ordering::SeqCst);
        holder.join().unwrap();
    });
}

fn segment_of(w: &World) -> String {
    w.rt.project_id.clone()
}

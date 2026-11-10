mod common;
use std::cell::RefCell;
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime};

use capstan_cache::{
    measure, over_cap, prune_with_occupants, read_manifest, recover, set_after_lock_open_hook,
    CacheError, Occupant, Policy, PruneHooks, PruneReport, Runtime, Skipped, BUILDING_REASON,
    IN_PROCESS_REASON, NOT_IDLE_REASON, NO_UNITS_REASON, OCCUPANT_REASON, RECENT_REASON,
    TOOL_TRIMMED_REASON,
};
use capstan_config::{CacheEntry, CacheOrigin, SetupReuse};
use common::*;

/// A child inherits every descriptor open in the process until it executes, and an `flock` lasts as long as any copy
/// of its descriptor is open. A test that spawns a process while another test holds a store lock would therefore
/// leave that lock held in the child for a moment, and the other test's prune would see a busy entry. Tests that
/// spawn run alone (write side); every other test shares the read side.
static SPAWN: RwLock<()> = RwLock::new(());

fn shared() -> RwLockReadGuard<'static, ()> {
    SPAWN.read().unwrap_or_else(|e| e.into_inner())
}

fn exclusive() -> RwLockWriteGuard<'static, ()> {
    SPAWN.write().unwrap_or_else(|e| e.into_inner())
}

const LOCK: &str = ".cargo-lock";
const HOURS: u64 = 3600;

fn rust_entry(mode: Option<&str>, gated: bool) -> CacheEntry {
    let mut e = custom("rust", "CARGO_TARGET_DIR", "cargo-target");
    e.origin = CacheOrigin::Preset;
    e.unit_lock = Some(LOCK.into());
    e.mode = mode.map(str::to_string);
    if mode == Some("sccache") {
        e.dirs.push(("SCCACHE_DIR".into(), "sccache".into()));
    }
    if mode == Some("shared-build-dir") {
        e.dirs
            .push(("CARGO_BUILD_BUILD_DIR".into(), "cargo-build".into()));
    }
    if gated {
        e.gate_dirs = vec![("CARGO_TARGET_DIR".into(), "gate-target".into())];
    }
    e
}

fn store(dir: &Path, entries: Vec<CacheEntry>) -> Runtime {
    let mut cfg = config(&dir.join("base"));
    cfg.entries = entries;
    let rt = runtime(&cfg, &dir.join("proj"));
    rt.ensure().unwrap();
    rt
}

fn rust_dir(rt: &Runtime) -> PathBuf {
    rt.root.join("rust")
}

/// `<entry>/<rel>` with its lock file and the given sparse files (name, bytes).
fn unit(rt: &Runtime, rel: &str, files: &[(&str, u64)]) -> PathBuf {
    let dir = rust_dir(rt).join(rel);
    fs::create_dir_all(&dir).unwrap();
    File::create(dir.join(LOCK)).unwrap();
    for (name, bytes) in files {
        sparse(&dir.join(name), *bytes);
    }
    dir
}

/// Sets the modification time of everything under `path` (and itself) to `seconds` ago.
fn age_tree(path: &Path, seconds: u64) {
    let when = SystemTime::now() - Duration::from_secs(seconds);
    let mut stack = vec![path.to_path_buf()];
    let mut all = Vec::new();
    while let Some(p) = stack.pop() {
        if fs::symlink_metadata(&p).unwrap().is_dir() {
            for child in fs::read_dir(&p).unwrap().flatten() {
                stack.push(child.path());
            }
        }
        all.push(p);
    }
    for p in all {
        File::open(&p).unwrap().set_modified(when).unwrap();
    }
}

fn policy(max: u64, idle: bool) -> Policy {
    Policy {
        max_bytes: max,
        max_age_ms: 3650 * 86_400_000,
        idle,
    }
}

fn run(rt: &Runtime, p: Policy, occupants: Option<&[Occupant]>) -> PruneReport {
    run_hooked(rt, p, occupants, &PruneHooks::default())
}

fn run_hooked(
    rt: &Runtime,
    p: Policy,
    occupants: Option<&[Occupant]>,
    hooks: &PruneHooks<'_>,
) -> PruneReport {
    for _ in 0..400 {
        match prune_with_occupants(rt, p, &env(&[]), &ok_runner, hooks, occupants) {
            Err(CacheError::Busy) => std::thread::sleep(Duration::from_millis(10)),
            other => return other.unwrap(),
        }
    }
    panic!("store stayed busy")
}

fn skipped(label: &str, reason: &str) -> Skipped {
    Skipped {
        label: label.into(),
        reason: reason.into(),
    }
}

/// A cap that is met once every unit is gone: what lies outside them, plus one byte.
fn cap_after_units(rt: &Runtime) -> u64 {
    measure(rt, Instant::now() + Duration::from_secs(60))
        .iter()
        .map(|s| s.fixed_bytes)
        .sum::<u64>()
        + 1
}

/// Takes the lock like a build tool would, or the errno.
fn flock(path: &Path, blocking: bool) -> Result<File, i32> {
    let file = File::options().read(true).write(true).open(path).unwrap();
    let flags = libc::LOCK_EX | if blocking { 0 } else { libc::LOCK_NB };
    // SAFETY: the descriptor is open for the lifetime of `file`.
    if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
        Ok(file)
    } else {
        Err(std::io::Error::last_os_error().raw_os_error().unwrap())
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|c| c.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn incremental_goes_first_then_the_oldest_unit_and_the_total_is_exact() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(
        dir.path(),
        vec![rust_entry(None, false), custom("other", "O_DIR", "d")],
    );
    unit(
        &rt,
        "cargo-target/debug",
        &[("deps/lib", 2 * GB), ("incremental/s/blob", GB)],
    );
    age_tree(&rust_dir(&rt).join("cargo-target/debug"), 3 * HOURS);
    unit(&rt, "cargo-target/release", &[("deps/lib", GB)]);
    sparse(&rt.root.join("other/d/blob"), GB / 10);
    let report = run(&rt, policy(2 * GB, true), None);
    assert_eq!(
        report.cleared,
        [
            "rust/cargo-target/debug/incremental",
            "rust/cargo-target/debug"
        ]
    );
    assert_eq!(report.total_before, 4 * GB + GB / 10);
    assert_eq!(report.total_after, GB + GB / 10);
    let debug = rust_dir(&rt).join("cargo-target/debug");
    assert_eq!(names(&debug), [LOCK], "only the lock file stays");
    assert!(rust_dir(&rt).join("cargo-target/release/deps/lib").exists());
}

#[test]
fn a_unit_whose_lock_is_held_is_skipped_as_building_and_never_renamed() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    let held = flock(&debug.join(LOCK), false).unwrap();
    let report = run(&rt, policy(2 * GB, true), None);
    assert!(report.cleared.is_empty(), "{report:?}");
    assert!(report
        .skipped
        .contains(&skipped("rust/cargo-target/debug", BUILDING_REASON)));
    assert!(debug.join("deps/lib").exists());
    drop(held);
    let report = run(&rt, policy(2 * GB, true), None);
    assert_eq!(report.cleared, ["rust/cargo-target/debug"]);
    assert!(!debug.join("deps/lib").exists());
}

#[test]
fn while_agents_run_old_units_and_unleased_slots_go_and_the_rest_is_listed() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(
        dir.path(),
        vec![rust_entry(None, true), custom("plain", "P_DIR", "d")],
    );
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    age_tree(&debug, 3 * HOURS);
    unit(&rt, "cargo-target/release", &[("deps/lib", GB)]);
    sparse(
        &rust_dir(&rt).join("gate-target/proj/slot-0/debug/blob"),
        GB,
    );
    let abort = AtomicBool::new(false);
    let leases = rt.lease_gate(1, Duration::ZERO, &abort).unwrap();
    sparse(
        &rust_dir(&rt).join("gate-target/proj/slot-1/debug/blob"),
        GB,
    );
    sparse(&rt.root.join("plain/d/blob"), 2 * GB);
    let report = run(&rt, policy(GB, false), None);
    assert_eq!(
        report.cleared,
        ["rust/proj/slot-0", "rust/cargo-target/debug"]
    );
    assert!(report
        .skipped
        .contains(&skipped("rust/cargo-target/release", RECENT_REASON)));
    assert!(report
        .skipped
        .contains(&skipped("rust/proj/slot-1", "in use")));
    assert!(report.skipped.contains(&skipped("plain", NOT_IDLE_REASON)));
    assert!(rt.root.join("plain/d/blob").exists(), "a whole entry stays");
    assert!(rust_dir(&rt).join("gate-target/proj/slot-1").exists());
    drop(leases);
}

#[test]
fn a_prune_without_occupants_records_what_it_skipped() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    unit(&rt, "cargo-target/release", &[("deps/lib", 2 * GB)]);
    let report = run(&rt, policy(GB, false), None);
    assert!(!report.skipped.is_empty(), "{report:?}");
    let manifest = read_manifest(&rt.root).unwrap().unwrap();
    assert_eq!(manifest.last_skipped, report.skipped);
}

#[test]
fn an_entry_without_a_unit_lock_has_no_unit_eviction() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut plain = rust_entry(None, false);
    plain.unit_lock = None;
    let mut rt = store(dir.path(), vec![plain]);
    rt.max_bytes = GB;
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    age_tree(&debug, 3 * HOURS);
    let report = run(&rt, policy(GB, false), None);
    assert!(report.cleared.is_empty());
    assert!(report.skipped.contains(&skipped("rust", NOT_IDLE_REASON)));
    assert!(debug.join("deps/lib").exists());
    let sizes = measure(&rt, Instant::now() + Duration::from_secs(60));
    assert!(sizes[0].units.is_empty());
    let over = over_cap(&rt, &sizes, 0);
    assert_eq!(over.bytes, 2 * GB);
    assert_eq!(over.not_enforced, [skipped("rust", NO_UNITS_REASON)]);
}

#[test]
fn occupants_count_against_the_cap_and_are_never_removed() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    for (name, age) in [("a", 5), ("b", 4), ("c", 3)] {
        let u = unit(&rt, &format!("cargo-target/{name}"), &[("deps/lib", GB)]);
        age_tree(&u, age * HOURS);
    }
    let occupant = [Occupant {
        label: "verify/r1".into(),
        bytes: 3 * GB,
    }];
    let report = run(&rt, policy(4 * GB, true), Some(&occupant));
    assert_eq!(report.total_after, GB, "{report:?}");
    assert_eq!(report.cleared.len(), 2);
    assert!(!report.skipped.iter().any(|s| s.reason == OCCUPANT_REASON));

    let rt = store(&dir.path().join("two"), vec![rust_entry(None, false)]);
    for name in ["a", "b", "c"] {
        unit(&rt, &format!("cargo-target/{name}"), &[("deps/lib", GB)]);
    }
    let occupant = [Occupant {
        label: "verify/r2".into(),
        bytes: 5 * GB,
    }];
    let report = run(&rt, policy(4 * GB, true), Some(&occupant));
    assert_eq!(report.total_after, 0);
    assert_eq!(report.cleared.len(), 3);
    assert!(report
        .skipped
        .contains(&skipped("verify/r2", OCCUPANT_REASON)));
}

#[test]
fn a_gate_blocked_on_the_lock_gets_the_same_inode_and_an_empty_unit() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    let lock_path = debug.join(LOCK);
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (opened_tx, opened_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel::<()>();
    let waiter = {
        let lock_path = lock_path.clone();
        std::thread::spawn(move || {
            go_rx.recv().unwrap();
            let file = File::options()
                .read(true)
                .write(true)
                .open(&lock_path)
                .unwrap();
            let inode = file.metadata().unwrap().ino();
            opened_tx.send(()).unwrap();
            // SAFETY: the descriptor is open for the lifetime of `file`.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            done_tx.send((rc, inode)).unwrap();
            let _ = finish_rx.recv();
        })
    };
    let hook = |_: &Path| {
        go_tx.send(()).unwrap();
        opened_rx.recv_timeout(Duration::from_secs(10)).unwrap()
    };
    let hooks = PruneHooks {
        before_rename: &|_| {},
        after_rename: &|| {},
        before_unit_rename: &hook,
    };
    let report = run_hooked(&rt, policy(GB, true), None, &hooks);
    assert_eq!(report.cleared, ["rust/cargo-target/debug"]);
    let (rc, inode) = done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(rc, 0, "the blocked locker got the lock");
    assert_eq!(inode, fs::metadata(&lock_path).unwrap().ino());
    assert_eq!(names(&debug), [LOCK]);
    finish_tx.send(()).unwrap();
    waiter.join().unwrap();
}

#[test]
fn the_active_rule_is_checked_again_under_the_lock() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    age_tree(&debug, 3 * HOURS);
    let touch = |unit_dir: &Path| {
        File::open(unit_dir.join("deps/lib"))
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
    };
    let hooks = PruneHooks {
        before_rename: &|_| {},
        after_rename: &|| {},
        before_unit_rename: &touch,
    };
    let report = run_hooked(&rt, policy(GB, false), None, &hooks);
    assert!(report.cleared.is_empty());
    assert!(report
        .skipped
        .contains(&skipped("rust/cargo-target/debug", RECENT_REASON)));
    assert!(debug.join("deps/lib").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn a_process_running_from_a_unit_keeps_it_until_it_exits() {
    let _serial = exclusive();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    age_tree(&debug, 3 * HOURS);
    let child: RefCell<Option<Child>> = RefCell::new(None);
    let start = |unit_dir: &Path| {
        *child.borrow_mut() = Some(
            Command::new("sleep")
                .arg("120")
                .current_dir(unit_dir)
                .stdin(Stdio::null())
                .spawn()
                .unwrap(),
        );
    };
    let hooks = PruneHooks {
        before_rename: &|_| {},
        after_rename: &|| {},
        before_unit_rename: &start,
    };
    let report = run_hooked(&rt, policy(GB, false), None, &hooks);
    assert!(report.cleared.is_empty(), "{report:?}");
    assert!(report
        .skipped
        .contains(&skipped("rust/cargo-target/debug", IN_PROCESS_REASON)));
    assert!(debug.join("deps/lib").exists());
    let mut running = child.borrow_mut().take().unwrap();
    running.kill().unwrap();
    running.wait().unwrap();
    let report = run(&rt, policy(GB, false), None);
    assert_eq!(report.cleared, ["rust/cargo-target/debug"], "{report:?}");
}

#[test]
fn the_measured_case_evicts_the_slot_and_the_stale_debug_unit() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(Some("sccache"), true)]);
    let big = GB * 5 / 2;
    sparse(&rust_dir(&rt).join("sccache/x/blob"), big);
    sparse(
        &rust_dir(&rt).join("gate-target/proj/slot-0/debug/blob"),
        big,
    );
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", GB * 39 / 10)]);
    age_tree(&debug, 3 * HOURS);
    let report = run(&rt, policy(5 * GB, false), None);
    assert_eq!(
        report.cleared,
        ["rust/proj/slot-0", "rust/cargo-target/debug"]
    );
    assert_eq!(report.total_before, big + big + GB * 39 / 10);
    assert_eq!(report.total_after, big);
    assert!(rust_dir(&rt).join("sccache/x/blob").exists());
}

#[test]
fn without_evict_while_active_nothing_is_removed_and_everything_is_listed() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut rt = store(dir.path(), vec![rust_entry(None, false)]);
    rt.evict_while_active = false;
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    age_tree(&debug, 3 * HOURS);
    let report = run(&rt, policy(GB, false), None);
    assert!(report.cleared.is_empty());
    assert_eq!(report.would_clear, ["rust/cargo-target/debug"]);
    assert!(debug.join("deps/lib").exists());
}

#[test]
fn a_cross_compile_unit_is_evicted_and_an_empty_unit_entry_is_left_alone() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    let triple = "cargo-target/x86_64-unknown-linux-gnu/debug";
    let unit_dir = unit(&rt, triple, &[("deps/lib", 3 * GB)]);
    unit(&rt, "cargo-target/release", &[("deps/lib", GB / 2)]);
    age_tree(&unit_dir, HOURS);
    let report = run(&rt, policy(GB, true), None);
    assert_eq!(report.cleared, [format!("rust/{triple}")]);
    assert!(rust_dir(&rt).is_dir() && rust_dir(&rt).join("cargo-target/release").is_dir());
}

#[test]
fn the_order_is_expired_then_slots_and_units_then_keys_then_entries() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    cfg.entries = vec![rust_entry(None, true), custom("plain", "P_DIR", "d")];
    cfg.setup_reuse = Some(SetupReuse {
        key_files: vec!["k".into()],
        paths: vec!["p".into()],
    });
    let rt = runtime(&cfg, &dir.path().join("proj"));
    rt.ensure().unwrap();
    let old = unit(&rt, "cargo-target/old", &[("deps/lib", GB)]);
    age_tree(&old, 20 * 86_400);
    let fresh = unit(&rt, "cargo-target/fresh", &[("deps/lib", GB)]);
    age_tree(&fresh, 3 * HOURS);
    sparse(&rt.root.join("setup/key/p/blob"), GB);
    sparse(&rt.root.join("plain/d/blob"), GB);
    age(&rt.root.join("setup/key/.last-used"), 60);
    let mut p = policy(0, true);
    p.max_age_ms = 10 * 86_400_000;
    let report = run(&rt, p, None);
    assert_eq!(
        report.cleared,
        [
            "rust/cargo-target/old",
            "rust/cargo-target/fresh",
            "setup/key",
            "plain"
        ]
    );
}

#[test]
fn an_unleased_slot_with_two_generations_goes_whole_and_a_leased_one_stays() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, true)]);
    let slot0 = rust_dir(&rt).join("gate-target/proj/slot-0");
    sparse(&slot0.join("debug/deps/old-debug-info"), GB);
    sparse(&slot0.join("debug/deps/new"), GB);
    let abort = AtomicBool::new(false);
    let leases = rt.lease_gate(1, Duration::ZERO, &abort).unwrap();
    sparse(
        &rust_dir(&rt).join("gate-target/proj/slot-1/debug/blob"),
        GB,
    );
    let report = run(&rt, policy(1, true), None);
    assert_eq!(report.cleared, ["rust/proj/slot-0"], "{report:?}");
    assert!(!slot0.exists());
    assert!(report
        .skipped
        .contains(&skipped("rust/proj/slot-1", "in use")));
    assert!(rust_dir(&rt)
        .join("gate-target/proj/slot-1/debug/blob")
        .exists());
    drop(leases);
}

#[test]
fn recover_removes_a_leftover_trash() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    sparse(
        &rt.root.join(".trash/rust_cargo-target_debug-1-0/deps/lib"),
        GB,
    );
    let removed = recover(&rt.root).unwrap();
    assert_eq!(removed, [".trash/rust_cargo-target_debug-1-0"]);
    assert!(names(&rt.root.join(".trash")).is_empty());
}

#[test]
fn sccache_bytes_are_fixed_and_units_give_way_to_fit_the_rest() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let mut rt = store(dir.path(), vec![rust_entry(Some("sccache"), false)]);
    rt.max_bytes = 2 * GB;
    sparse(&rust_dir(&rt).join("sccache/x/blob"), 3 * GB);
    let sizes = measure(&rt, Instant::now() + Duration::from_secs(60));
    let over = over_cap(&rt, &sizes, 0);
    assert_eq!(over.bytes, GB);
    assert_eq!(over.not_enforced, [skipped("rust", TOOL_TRIMMED_REASON)]);
    let report = run(&rt, policy(2 * GB, false), None);
    assert!(report.cleared.is_empty(), "no unit, so nothing goes alone");
    assert!(report.skipped.contains(&skipped("rust", NOT_IDLE_REASON)));
    assert!(rust_dir(&rt).join("sccache/x/blob").exists());

    let dir = tempfile::tempdir().unwrap();
    let mut rt = store(dir.path(), vec![rust_entry(Some("sccache"), false)]);
    rt.max_bytes = 2 * GB;
    sparse(&rust_dir(&rt).join("sccache/x/blob"), GB);
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", GB * 3 / 2)]);
    age_tree(&debug, 3 * HOURS);
    let report = run(&rt, policy(2 * GB, true), None);
    assert_eq!(report.cleared, ["rust/cargo-target/debug"]);
    assert_eq!(report.total_after, GB);
    let sizes = measure(&rt, Instant::now() + Duration::from_secs(60));
    assert_eq!(over_cap(&rt, &sizes, 0), Default::default());
}

#[test]
fn a_split_build_dir_pair_is_emptied_together_or_not_at_all() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(
        dir.path(),
        vec![rust_entry(Some("shared-build-dir"), false)],
    );
    let artifact = unit(&rt, "cargo-target/debug", &[("deps/lib", GB)]);
    let build = unit(&rt, "cargo-build/debug", &[("deps/obj", 2 * GB)]);
    age_tree(&artifact, 3 * HOURS);
    age_tree(&build, 3 * HOURS);
    let held = flock(&build.join(LOCK), false).unwrap();
    let report = run(&rt, policy(GB, false), None);
    assert!(report.cleared.is_empty(), "{report:?}");
    assert!(artifact.join("deps/lib").exists() && build.join("deps/obj").exists());
    assert!(report
        .skipped
        .contains(&skipped("rust/cargo-build/debug", BUILDING_REASON)));
    drop(held);
    let report = run(&rt, policy(GB, false), None);
    assert_eq!(report.cleared.len(), 2, "{report:?}");
    assert_eq!(names(&artifact), [LOCK]);
    assert_eq!(names(&build), [LOCK]);
}

// ---- real cargo -------------------------------------------------------------------------------------------------

const BUILD_RS: &str = r#"
use std::{env, fs, path::Path, time::{Duration, Instant}};
fn main() {
    println!("cargo:rerun-if-env-changed=EVICT_MARKER");
    println!("cargo:rerun-if-env-changed=EVICT_RELEASE");
    println!("cargo:rerun-if-env-changed=EVICT_FAIL");
    if let Ok(marker) = env::var("EVICT_MARKER") {
        fs::write(marker, b"started").unwrap();
    }
    if let Ok(release) = env::var("EVICT_RELEASE") {
        let end = Instant::now() + Duration::from_secs(60);
        while !Path::new(&release).exists() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if env::var("EVICT_FAIL").is_ok() {
        std::process::exit(1);
    }
}
"#;

fn probe_crate(dir: &Path) -> PathBuf {
    let root = dir.join("probe");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"evict-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();
    fs::write(root.join("build.rs"), BUILD_RS).unwrap();
    root
}

fn cargo_build(cargo: &str, probe: &Path, target: &Path, extra: &[(&str, &Path)]) -> Child {
    let mut cmd = Command::new(cargo);
    cmd.args(["build", "--offline", "--quiet"])
        .current_dir(probe)
        .env("CARGO_TARGET_DIR", target)
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("EVICT_MARKER")
        .env_remove("EVICT_RELEASE")
        .env_remove("EVICT_FAIL")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, value) in extra {
        cmd.env(name, value);
    }
    cmd.spawn().unwrap()
}

/// Waits for `child`, killing it (and failing) after `secs`.
fn finish(mut child: Child, secs: u64, what: &str) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.success();
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{what} did not finish in {secs} s");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for(path: &Path, child: &mut Child, secs: u64) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !path.exists() {
        if child.try_wait().unwrap().is_some() || Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{} never appeared", path.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn cargo_or_skip() -> Option<String> {
    match std::env::var("CARGO") {
        Ok(cargo) => Some(cargo),
        Err(_) => {
            eprintln!("skipped: CARGO is not set, so there is no cargo to run");
            None
        }
    }
}

fn cargo_store(dir: &Path) -> (Runtime, PathBuf) {
    let rt = store(dir, vec![rust_entry(None, false)]);
    let target = rust_dir(&rt).join("cargo-target");
    (rt, target)
}

#[test]
fn real_cargo_holds_the_lock_where_prune_looks_and_is_never_evicted_under() {
    let _serial = exclusive();
    let Some(cargo) = cargo_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (rt, target) = cargo_store(dir.path());
    let probe = probe_crate(dir.path());
    let marker = dir.path().join("marker");
    let release = dir.path().join("release");
    let mut child = cargo_build(
        &cargo,
        &probe,
        &target,
        &[
            ("EVICT_MARKER", &marker),
            ("EVICT_RELEASE", &release),
            ("EVICT_FAIL", &marker),
        ],
    );
    wait_for(&marker, &mut child, 90);
    let lock = target.join("debug").join(LOCK);
    assert_eq!(flock(&lock, false).err(), Some(libc::EWOULDBLOCK));
    let report = run(&rt, policy(cap_after_units(&rt), true), None);
    assert!(report
        .skipped
        .contains(&skipped("rust/cargo-target/debug", BUILDING_REASON)));
    assert!(!report
        .cleared
        .contains(&"rust/cargo-target/debug".to_string()));
    fs::write(&release, b"go").unwrap();
    assert!(!finish(child, 90, "the failing build"), "build.rs fails");
}

#[cfg(target_os = "linux")]
fn waiting_on(inode: u64, pid: u32) -> bool {
    // A blocked flock waiter shows as `N: -> FLOCK ADVISORY WRITE <pid> <maj:min:inode> ...`.
    let Ok(text) = fs::read_to_string("/proc/locks") else {
        return false;
    };
    let suffix = format!(":{inode} ");
    text.lines().any(|line| {
        line.contains("->") && line.contains(&format!(" {pid} ")) && line.contains(&suffix)
    })
}

#[cfg(target_os = "linux")]
#[test]
fn a_cargo_blocked_on_the_lock_proceeds_cold_after_the_eviction() {
    let _serial = exclusive();
    let Some(cargo) = cargo_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    let (rt, target) = cargo_store(dir.path());
    let probe = probe_crate(dir.path());
    assert!(finish(
        cargo_build(&cargo, &probe, &target, &[]),
        90,
        "the first build"
    ));
    let debug = target.join("debug");
    assert!(debug.join(LOCK).is_file());
    age_tree(&debug, 3 * HOURS);
    let inode = fs::metadata(debug.join(LOCK)).unwrap().ino();
    let second: RefCell<Option<Child>> = RefCell::new(None);
    let hook = |_: &Path| {
        let mut child = cargo_build(&cargo, &probe, &target, &[]);
        let end = Instant::now() + Duration::from_secs(90);
        while !waiting_on(inode, child.id()) {
            assert!(
                child.try_wait().unwrap().is_none(),
                "the second build exited while prune held the lock"
            );
            if Instant::now() >= end {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the second build never blocked on the lock");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(child.try_wait().unwrap().is_none());
        *second.borrow_mut() = Some(child);
    };
    let hooks = PruneHooks {
        before_rename: &|_| {},
        after_rename: &|| {},
        before_unit_rename: &hook,
    };
    let cap = cap_after_units(&rt);
    let report = run_hooked(&rt, policy(cap, true), None, &hooks);
    assert_eq!(report.cleared, ["rust/cargo-target/debug"], "{report:?}");
    let second = second.borrow_mut().take().unwrap();
    assert!(finish(second, 120, "the blocked build"), "it builds cold");
    assert!(finish(
        cargo_build(&cargo, &probe, &target, &[]),
        90,
        "the third build"
    ));
    assert_eq!(fs::metadata(debug.join(LOCK)).unwrap().ino(), inode);
}

#[test]
fn a_lock_file_replaced_between_open_and_lock_counts_as_building() {
    let _serial = shared();
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path(), vec![rust_entry(None, false)]);
    let debug = unit(&rt, "cargo-target/debug", &[("deps/lib", 3 * GB)]);
    age_tree(&debug, 3 * HOURS);
    // A build recreates the lock file after the prune opened the old one.
    set_after_lock_open_hook(Some(Box::new(|path: &Path| {
        fs::remove_file(path).unwrap();
        File::create(path).unwrap();
    })));
    let report = run_hooked(&rt, policy(GB, true), None, &PruneHooks::default());
    set_after_lock_open_hook(None);
    assert!(report.cleared.is_empty(), "{report:?}");
    assert!(report
        .skipped
        .contains(&skipped("rust/cargo-target/debug", BUILDING_REASON)));
    assert!(debug.join("deps/lib").exists());
}

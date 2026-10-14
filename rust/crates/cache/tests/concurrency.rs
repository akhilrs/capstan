mod common;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use capstan_cache::{
    clear_hooked, read_manifest, recover, write_manifest_hooked, Manifest, Runtime, StoreLock,
};
use common::*;

fn build(base: &Path, project: &Path) -> Runtime {
    let mut cfg = config(base);
    cfg.entries = vec![custom("a", "A_DIR", "x"), custom("b", "B_DIR", "y")];
    runtime(&cfg, project)
}

/// A forked child holds a copy of every descriptor the parent has open until it execs, and a copy
/// of a held `flock` keeps the lock held. Tests run as threads of one process, so a spawn racing
/// another test's lock would leak that lock into the child for as long as load delays the exec.
/// Spawning and every parent-side hold of the store lock take this mutex, so they never overlap.
static SPAWN_VS_LOCK: Mutex<()> = Mutex::new(());

fn while_not_spawning<T>(f: impl FnOnce() -> T) -> T {
    let _guard = SPAWN_VS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// Re-runs this test binary as a child that executes the named `child_*` test.
fn spawn_child(test: &str, base: &Path, project: &Path, marker: &Path) -> std::process::Child {
    let _guard = SPAWN_VS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env("CACHE_CHILD_BASE", base)
        .env("CACHE_CHILD_PROJECT", project)
        .env("CACHE_CHILD_MARKER", marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn child_env() -> Option<(Runtime, std::path::PathBuf)> {
    let base = std::env::var_os("CACHE_CHILD_BASE")?;
    let project = std::env::var_os("CACHE_CHILD_PROJECT")?;
    let marker = std::env::var_os("CACHE_CHILD_MARKER")?;
    Some((build(Path::new(&base), Path::new(&project)), marker.into()))
}

#[test]
fn child_ensure() {
    if let Some((rt, _)) = child_env() {
        for _ in 0..20 {
            rt.ensure().unwrap();
        }
    }
}

#[test]
fn child_clear_stalls_after_rename() {
    if let Some((rt, marker)) = child_env() {
        let hook = || {
            fs::write(&marker, "renamed").unwrap();
            signal_ready();
            std::thread::sleep(Duration::from_secs(120));
        };
        let _ = clear_hooked(&rt, None, true, &env(&[]), &ok_runner, &hook);
    }
}

#[test]
fn child_manifest_stalls_before_rename() {
    if let Some((rt, marker)) = child_env() {
        let manifest = Manifest::new(&rt, Vec::new(), 222);
        let hook = || {
            fs::write(&marker, "written").unwrap();
            signal_ready();
            std::thread::sleep(Duration::from_secs(120));
        };
        let _ = write_manifest_hooked(&rt, &manifest, &hook);
    }
}

const READY: &str = "CAPSTAN-CHILD-READY";

/// Tells the parent the child reached its stall point.
fn signal_ready() {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    writeln!(out, "\n{READY}").unwrap();
    out.flush().unwrap();
}

/// Blocks until the child reports it reached its stall point. No deadline: the
/// read ends when the child signals or exits, so load cannot make it flaky.
fn wait_ready(child: &mut std::process::Child) {
    use std::io::{BufRead, BufReader};
    let out = child.stdout.take().expect("child stdout is piped");
    for line in BufReader::new(out).lines() {
        if line.unwrap().contains(READY) {
            return;
        }
    }
    panic!("the child exited before reaching its stall point");
}

#[test]
fn ensure_from_threads_and_processes_at_once_leaves_one_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (base, project) = (dir.path().join("base"), dir.path().join("proj"));
    let rt = build(&base, &project);
    let marker = dir.path().join("m");
    let mut children: Vec<_> = (0..4)
        .map(|_| spawn_child("child_ensure", &base, &project, &marker))
        .collect();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..20 {
                    rt.ensure().unwrap();
                }
            });
        }
    });
    for child in &mut children {
        assert!(child.wait().unwrap().success());
    }
    let mut expected: Vec<String> = ["a", "a/.last-used", "a/x", "b", "b/.last-used", "b/y"]
        .map(String::from)
        .to_vec();
    expected.push(".lock".into());
    let mut found = tree(&rt.root);
    found.retain(|p| p != ".lock");
    expected.retain(|p| p != ".lock");
    assert_eq!(found, expected);
}

#[test]
fn a_clear_killed_between_rename_and_delete_is_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let (base, project) = (dir.path().join("base"), dir.path().join("proj"));
    let rt = build(&base, &project);
    rt.ensure().unwrap();
    fs::write(rt.root.join("a/x/data"), "payload").unwrap();
    let marker = dir.path().join("renamed");
    let mut child = spawn_child("child_clear_stalls_after_rename", &base, &project, &marker);
    wait_ready(&mut child);
    child.kill().unwrap();
    child.wait().unwrap();
    let trash: Vec<_> = fs::read_dir(rt.root.join(".trash")).unwrap().collect();
    assert!(!trash.is_empty(), "the renamed entry waits in .trash");
    let removed = while_not_spawning(|| recover(&rt.root)).unwrap();
    assert!(!removed.is_empty());
    assert_eq!(fs::read_dir(rt.root.join(".trash")).unwrap().count(), 0);
    rt.ensure().unwrap();
    assert!(rt.root.join("a/x").is_dir() && !rt.root.join("a/x/data").exists());
    while_not_spawning(|| StoreLock::try_acquire(&rt.root))
        .expect("the killed process released the lock");
}

#[test]
fn a_manifest_write_killed_midway_leaves_the_old_manifest_readable() {
    let dir = tempfile::tempdir().unwrap();
    let (base, project) = (dir.path().join("base"), dir.path().join("proj"));
    let rt = build(&base, &project);
    rt.ensure().unwrap();
    let old = Manifest::new(&rt, Vec::new(), 111);
    while_not_spawning(|| capstan_cache::write_manifest(&rt, &old)).unwrap();
    let marker = dir.path().join("written");
    let mut child = spawn_child(
        "child_manifest_stalls_before_rename",
        &base,
        &project,
        &marker,
    );
    wait_ready(&mut child);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(
        read_manifest(&rt.root).unwrap().unwrap().measured_at_ms,
        111
    );
    assert!(fs::read_dir(&rt.root)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".tmp-")));
    while_not_spawning(|| recover(&rt.root)).unwrap();
    assert!(!fs::read_dir(&rt.root)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains(".tmp-")));
    assert_eq!(
        read_manifest(&rt.root).unwrap().unwrap().measured_at_ms,
        111
    );
}

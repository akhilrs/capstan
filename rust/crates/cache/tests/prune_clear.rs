mod common;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use capstan_cache::{
    clear as clear_once, is_idle, measure, prune, setup_key, CacheError, Policy, Runtime,
    SETUP_ENTRY,
};
use capstan_config::{CacheEntry, CacheOrigin, SetupReuse};
use common::*;

/// A lock fd can outlive its holder for an instant in a child another test thread forked; `Busy` then is transient.
fn clear(
    rt: &Runtime,
    sel: Option<&[String]>,
    force: bool,
    base: &BTreeMap<String, String>,
    runner: capstan_cache::Runner<'_>,
) -> Result<capstan_cache::ClearReport, CacheError> {
    for _ in 0..200 {
        match clear_once(rt, sel, force, base, runner) {
            Err(CacheError::Busy) => std::thread::sleep(Duration::from_millis(10)),
            other => return other,
        }
    }
    clear_once(rt, sel, force, base, runner)
}

fn retry_busy<T>(mut call: impl FnMut() -> Result<T, CacheError>) -> Result<T, CacheError> {
    for _ in 0..200 {
        match call() {
            Err(CacheError::Busy) => std::thread::sleep(Duration::from_millis(10)),
            other => return other,
        }
    }
    call()
}

const MB: u64 = 1_000_000;
const YEAR: u64 = 365 * 86_400;

fn store(dir: &Path) -> Runtime {
    let mut cfg = config(&dir.join("base"));
    cfg.entries = vec![custom("a", "A_DIR", "d"), custom("b", "B_DIR", "d")];
    cfg.setup_reuse = Some(SetupReuse {
        key_files: vec!["k".into()],
        paths: vec!["p".into()],
    });
    let rt = runtime(&cfg, &dir.join("proj"));
    rt.ensure().unwrap();
    rt
}

fn fill(rt: &Runtime) {
    sparse(&rt.root.join("a/d/blob"), 300 * MB);
    sparse(&rt.root.join("b/d/blob"), 500 * MB);
    sparse(&rt.root.join("setup/old/p/blob"), 200 * MB);
    sparse(&rt.root.join("setup/new/p/blob"), 100 * MB);
    age(&rt.root.join("a/.last-used"), 3 * 86_400);
    age(&rt.root.join("setup/old/.last-used"), 5 * 86_400);
    age(&rt.root.join("setup/new/.last-used"), 60);
}

fn policy(max: u64, idle: bool) -> Policy {
    Policy {
        max_bytes: max,
        max_age_ms: 10 * YEAR * 1000,
        idle,
    }
}

fn run_prune(rt: &Runtime, p: Policy) -> capstan_cache::PruneReport {
    for _ in 0..200 {
        match prune(rt, p, &env(&[]), &ok_runner) {
            Err(CacheError::Busy) => std::thread::sleep(Duration::from_millis(10)),
            other => return other.unwrap(),
        }
    }
    panic!("store stayed busy")
}

#[test]
fn prune_stops_exactly_when_the_total_fits_the_cap() {
    // Total 1100 MB: a 300, b 500, setup/old 200, setup/new 100. Eviction order: old, new, a, b.
    let cases: [(u64, &[&str]); 5] = [
        (1100 * MB, &[]),
        (900 * MB, &["setup/old"]),
        (899 * MB, &["setup/old", "setup/new"]),
        (500 * MB, &["setup/old", "setup/new", "a"]),
        (499 * MB, &["setup/old", "setup/new", "a", "b"]),
    ];
    for (cap, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let rt = store(dir.path());
        fill(&rt);
        let report = run_prune(&rt, policy(cap, true));
        assert_eq!(report.cleared, expected, "cap {cap}");
        assert_eq!(report.total_before, 1100 * MB);
    }
}

#[test]
fn prune_while_not_idle_clears_nothing_and_reports_the_same_list() {
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path());
    fill(&rt);
    let report = run_prune(&rt, policy(500 * MB, false));
    assert!(report.cleared.is_empty());
    assert_eq!(report.would_clear, ["setup/old", "setup/new", "a"]);
    assert!(rt.root.join("setup/old").exists() && rt.root.join("a").exists());
    let idle = run_prune(&rt, policy(500 * MB, true));
    assert_eq!(idle.cleared, report.would_clear);
}

#[test]
fn prune_runs_tool_commands_and_clears_entries_older_than_the_max_age() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut tool: CacheEntry = custom("tool", "TOOL_DIR", "d");
    tool.origin = CacheOrigin::Preset;
    tool.prune = vec!["tool prune".into()];
    cfg.entries = vec![tool, custom("stale", "STALE_DIR", "d")];
    let rt = runtime(&cfg, dir.path());
    rt.ensure().unwrap();
    age(&rt.root.join("stale/.last-used"), 40 * 86_400);
    let seen = Mutex::new(Vec::new());
    let runner = |cmd: &str, cwd: &Path, e: &BTreeMap<String, String>, _| {
        seen.lock()
            .unwrap()
            .push((cmd.to_string(), cwd.to_path_buf(), e.clone()));
        capstan_cache::CommandOutcome {
            ok: true,
            message: String::new(),
        }
    };
    let p = Policy {
        max_bytes: GB,
        max_age_ms: 30 * 86_400_000,
        idle: true,
    };
    let report = prune(&rt, p, &env(&[("PATH", "/bin"), ("OTHER", "x")]), &runner).unwrap();
    assert_eq!(report.cleared, ["stale"]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "tool prune");
    assert_eq!(seen[0].1, rt.root.join("tool"));
    assert!(
        seen[0].2.contains_key("TOOL_DIR")
            && seen[0].2.contains_key("PATH")
            && !seen[0].2.contains_key("OTHER")
    );
    assert!(capstan_cache::read_manifest(&rt.root)
        .unwrap()
        .unwrap()
        .last_prune_at_ms
        .is_some());
}

#[test]
fn two_prunes_at_once_one_runs_and_the_other_is_busy() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut tool = custom("tool", "TOOL_DIR", "d");
    tool.prune = vec!["slow".into()];
    cfg.entries = vec![tool];
    let rt = runtime(&cfg, dir.path());
    rt.ensure().unwrap();
    let started = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            let runner = |_: &str, _: &Path, _: &BTreeMap<String, String>, _| {
                started.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(400));
                capstan_cache::CommandOutcome {
                    ok: true,
                    message: String::new(),
                }
            };
            prune(&rt, policy(GB, true), &env(&[]), &runner)
        });
        while !started.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let second = prune(&rt, policy(GB, true), &env(&[]), &ok_runner);
        assert!(matches!(second, Err(CacheError::Busy)));
        assert!(first.join().unwrap().is_ok());
    });
    let manifest = capstan_cache::read_manifest(&rt.root).unwrap();
    assert!(manifest.is_some(), "the manifest is whole after the run");
}

#[test]
fn is_idle_follows_linked_worktrees() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    repo(&project);
    assert!(is_idle(&project).unwrap());
    let wt = dir.path().join("wt");
    git(
        &project,
        &["worktree", "add", "-q", "-b", "agent", wt.to_str().unwrap()],
    );
    assert!(!is_idle(&project).unwrap());
    let mut setup = std::process::Command::new("sleep")
        .arg("1")
        .current_dir(&wt)
        .spawn()
        .unwrap();
    assert!(
        !is_idle(&project).unwrap(),
        "also while a setup command runs in the worktree"
    );
    setup.wait().unwrap();
    git(
        &project,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
    assert!(is_idle(&project).unwrap());
    assert!(!is_idle(&dir.path().join("not-a-repo")).unwrap());
    fs::create_dir_all(dir.path().join("plain")).unwrap();
    assert!(!is_idle(&dir.path().join("plain")).unwrap());
}

#[test]
fn clear_refuses_while_not_idle_unless_forced() {
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path());
    sparse(&rt.root.join("a/d/blob"), 10);
    assert!(matches!(
        clear(&rt, None, false, &env(&[]), &ok_runner),
        Err(CacheError::NotIdle)
    ));
    assert!(rt.root.join("a/d/blob").exists());
    let report = clear(&rt, Some(&["a".to_string()]), true, &env(&[]), &ok_runner).unwrap();
    assert_eq!(report.cleared, ["a"]);
    assert!(matches!(
        clear(
            &rt,
            Some(&["nope".to_string()]),
            true,
            &env(&[]),
            &ok_runner
        ),
        Err(CacheError::Invalid(_))
    ));
}

#[test]
fn clear_makes_read_only_trees_writable_and_ensure_recreates() {
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path());
    let ro = rt.root.join("a/d/mod/pkg");
    fs::create_dir_all(&ro).unwrap();
    fs::write(ro.join("f.go"), "x").unwrap();
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(rt.root.join("a/d/mod"), fs::Permissions::from_mode(0o555)).unwrap();
    clear(&rt, Some(&["a".to_string()]), true, &env(&[]), &ok_runner).unwrap();
    assert!(!rt.root.join("a").exists());
    assert_eq!(fs::read_dir(rt.root.join(".trash")).unwrap().count(), 0);
    rt.ensure().unwrap();
    assert!(rt.root.join("a/d").is_dir());
}

#[test]
fn clear_keeps_open_descriptors_working_and_never_follows_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path());
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep"), "keep").unwrap();
    std::os::unix::fs::symlink(&outside, rt.root.join("a/d/link")).unwrap();
    std::os::unix::fs::symlink(outside.join("keep"), rt.root.join("a/d/filelink")).unwrap();
    use std::io::Write;
    let mut writer = fs::File::create(rt.root.join("a/d/open.log")).unwrap();
    writer.write_all(b"before").unwrap();
    let report = clear(&rt, Some(&["a".to_string()]), true, &env(&[]), &ok_runner).unwrap();
    assert_eq!(report.cleared, ["a"]);
    writer
        .write_all(b" after")
        .expect("the open descriptor still works");
    writer.sync_all().unwrap();
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
    assert!(outside.is_dir());
    rt.ensure().unwrap();
    assert!(rt.root.join("a/d").is_dir() && !rt.root.join("a/d/open.log").exists());
}

#[test]
fn clear_runs_stop_commands_before_the_rename_with_the_entry_env() {
    let dir = tempfile::tempdir().unwrap();
    let t = dir.path();
    let bin = t.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = t.join("stop.log");
    let stub = bin.join("sccache");
    fs::write(&stub, format!("#!/bin/sh\necho \"$1 $SCCACHE_DIR\" >> {0}\ntest -d \"$SCCACHE_DIR\" && echo present >> {0}\n", log.display())).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = config(&t.join("base"));
    let mut rust = custom("rust", "SCCACHE_DIR", "sccache");
    rust.origin = CacheOrigin::Preset;
    rust.mode = Some("sccache".into());
    rust.requires = Some("sccache".into());
    rust.stop = vec!["sccache --stop-server".into()];
    cfg.entries = vec![rust];
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let rt = Runtime::from_config(&cfg, t, "p", &env(&[]), &path);
    rt.ensure().unwrap();
    let base = env(&[("PATH", &path)]);
    clear(&rt, Some(&["rust".to_string()]), true, &base, &shell_runner).unwrap();
    let text = fs::read_to_string(&log).unwrap();
    assert!(
        text.contains(&format!(
            "--stop-server {}",
            rt.root.join("rust/sccache").display()
        )),
        "{text}"
    );
    assert!(
        text.contains("present"),
        "the directory still existed when the command ran"
    );
    // sccache absent: the entry is off, clear proceeds without error.
    let absent = Runtime::from_config(&cfg, t, "p", &env(&[]), "/nonexistent");
    absent.ensure().unwrap();
    fs::create_dir_all(absent.root.join("rust")).unwrap();
    fs::write(absent.root.join("rust/f"), "x").unwrap();
    let report = clear(
        &absent,
        None,
        true,
        &env(&[("PATH", "/nonexistent")]),
        &shell_runner,
    )
    .unwrap();
    assert_eq!(report.cleared, ["rust"]);
    assert!(report.stop_commands.is_empty());
    // A failing stop command does not stop the clear.
    let mut failing = cfg.clone();
    failing.entries[0].requires = None;
    failing.entries[0].stop = vec!["exit 3".into()];
    let rt = Runtime::from_config(&failing, t, "p", &env(&[]), "/bin");
    rt.ensure().unwrap();
    let report = clear(&rt, None, true, &env(&[]), &shell_runner).unwrap();
    assert!(!report.stop_commands[0].ok);
    assert_eq!(report.cleared, ["rust"]);
}

#[test]
fn measure_respects_the_deadline_and_never_follows_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path());
    let outside = dir.path().join("outside");
    sparse(&outside.join("big"), 900 * MB);
    let entry = rt.root.join("a/d");
    std::os::unix::fs::symlink(&outside, entry.join("link")).unwrap();
    std::os::unix::fs::symlink(&entry, entry.join("loop")).unwrap();
    sparse(&entry.join("one"), 7);
    let sizes = measure(&rt, Instant::now() + Duration::from_secs(10));
    let a = sizes.iter().find(|s| s.name == "a").unwrap();
    assert!(!a.partial && a.files == 3 + 1, "{a:?}"); // one, two symlinks, .last-used
    assert!(a.bytes < 1000, "{a:?}");
    for i in 0..300 {
        for j in 0..100 {
            fs::create_dir_all(entry.join(format!("t{i}"))).unwrap();
            fs::write(entry.join(format!("t{i}/f{j}")), "x").unwrap();
        }
    }
    let started = Instant::now();
    let sizes = measure(&rt, started);
    assert!(sizes.iter().find(|s| s.name == "a").unwrap().partial);
    assert!(started.elapsed() < Duration::from_secs(2));
    let full = measure(&rt, Instant::now() + Duration::from_secs(60));
    assert_eq!(
        full.iter().find(|s| s.name == "a").unwrap().files,
        30_000 + 4
    );
}

#[test]
fn setup_snapshots_store_restore_and_the_first_key_wins() {
    let dir = tempfile::tempdir().unwrap();
    let rt = store(dir.path());
    let reuse = rt.setup_reuse.clone().unwrap();
    let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
    fs::create_dir_all(src.join("p/sub")).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("k"), "lock v1").unwrap();
    fs::write(src.join("p/sub/f"), "one").unwrap();
    std::os::unix::fs::symlink("sub/f", src.join("p/ln")).unwrap();
    let key = setup_key(&src, &reuse).unwrap().unwrap();
    fs::write(src.join("k"), "lock v2").unwrap();
    assert_ne!(setup_key(&src, &reuse).unwrap().unwrap(), key);
    assert!(!rt.restore_snapshot(&key, &dst, &reuse.paths).unwrap());
    rt.store_snapshot(&key, &src, &reuse.paths).unwrap();
    fs::write(src.join("p/sub/f"), "two").unwrap();
    rt.store_snapshot(&key, &src, &reuse.paths).unwrap();
    assert!(rt.restore_snapshot(&key, &dst, &reuse.paths).unwrap());
    assert_eq!(fs::read_to_string(dst.join("p/sub/f")).unwrap(), "one");
    assert_eq!(fs::read_link(dst.join("p/ln")).unwrap(), Path::new("sub/f"));
    assert!(rt.root.join(SETUP_ENTRY).join(&key).is_dir());
    assert!(rt
        .store_snapshot(&key, &src, &["../x".to_string()])
        .is_err());
    assert!(!tree(&rt.root.join("setup"))
        .iter()
        .any(|p| p.contains(".tmp-")));
    assert_eq!(setup_key(&dst, &reuse).unwrap(), None);
}

#[test]
fn clear_keeps_going_after_a_failed_victim_and_records_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir.path().join("base"));
    let mut b = custom("b", "B_DIR", "d");
    b.stop = vec!["unlock".into()];
    cfg.entries = vec![custom("a", "A_DIR", "d"), b];
    let rt = runtime(&cfg, dir.path());
    rt.ensure().unwrap();
    let sizes = measure(&rt, Instant::now() + Duration::from_secs(5));
    let manifest = capstan_cache::Manifest::new(&rt, sizes, 5);
    retry_busy(|| capstan_cache::write_manifest(&rt, &manifest)).unwrap();
    let trash = rt.root.join(".trash");
    let hook = |_: ()| fs::set_permissions(&trash, fs::Permissions::from_mode(0o500)).unwrap();
    let first = AtomicBool::new(true);
    // After a's rename the trash becomes read-only, so deleting it fails; b's stop command makes it writable again.
    let after = || {
        if first.swap(false, Ordering::SeqCst) {
            hook(());
        }
    };
    let runner = |_: &str, _: &Path, _: &BTreeMap<String, String>, _| {
        fs::set_permissions(&trash, fs::Permissions::from_mode(0o700)).unwrap();
        capstan_cache::CommandOutcome {
            ok: true,
            message: String::new(),
        }
    };
    let report =
        retry_busy(|| capstan_cache::clear_hooked(&rt, None, true, &env(&[]), &runner, &after))
            .unwrap();
    assert_eq!(report.cleared, ["b"]);
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(report.errors[0].starts_with("a:"));
    let manifest = capstan_cache::read_manifest(&rt.root).unwrap().unwrap();
    let names: Vec<&str> = manifest.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["a"], "the removed victim is recorded");
    retry_busy(|| capstan_cache::recover(&rt.root)).unwrap();
    assert_eq!(fs::read_dir(&trash).unwrap().count(), 0);
}

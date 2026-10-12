#![allow(dead_code)]
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use capstan_cache::{CommandOutcome, Runtime};
use capstan_config::{CacheConfig, CacheEntry, CacheOrigin, CacheScope};

pub const GB: u64 = 1_000_000_000;

pub fn config(base: &Path) -> CacheConfig {
    CacheConfig {
        base: Some(base.to_path_buf()),
        scope: CacheScope::Project,
        max_size_bytes: 20 * (1 << 30),
        max_age_days: 3650,
        detect: false,
        entries: Vec::new(),
        detectable: Vec::new(),
        setup_reuse: None,
    }
}

pub fn custom(name: &str, dir_env: &str, subdir: &str) -> CacheEntry {
    CacheEntry {
        name: name.into(),
        origin: CacheOrigin::Custom,
        dirs: vec![(dir_env.into(), subdir.into())],
        set: Vec::new(),
        requires: None,
        prune: Vec::new(),
        stop: Vec::new(),
        mode: None,
        max_size_bytes: None,
    }
}

pub fn preset(name: &str) -> CacheEntry {
    let mut entry = custom(name, "X", "x");
    entry.origin = CacheOrigin::Preset;
    entry
}

pub fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn runtime(config: &CacheConfig, project: &Path) -> Runtime {
    Runtime::from_config(
        config,
        project,
        "proj",
        &env(&[("HOME", "/nonexistent")]),
        "/usr/bin:/bin",
    )
}

pub fn ok_runner(_: &str, _: &Path, _: &BTreeMap<String, String>, _: u64) -> CommandOutcome {
    CommandOutcome {
        ok: true,
        message: String::new(),
    }
}

pub fn sparse(path: &Path, bytes: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(path).unwrap().set_len(bytes).unwrap();
}

pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

pub fn repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
}

/// Every path under `root` (relative), sorted, without following symlinks.
pub fn tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for child in fs::read_dir(&dir).unwrap().flatten() {
            let path = child.path();
            out.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                stack.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Runs the command through `sh -c` with exactly the given environment.
pub fn shell_runner(
    command: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    _: u64,
) -> CommandOutcome {
    let output = Command::new("sh")
        .args(["-c", command])
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .output();
    match output {
        Ok(out) => CommandOutcome {
            ok: out.status.success(),
            message: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) => CommandOutcome {
            ok: false,
            message: e.to_string(),
        },
    }
}

pub fn age(path: &Path, seconds: u64) {
    let file = fs::File::options()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(seconds))
        .unwrap();
}

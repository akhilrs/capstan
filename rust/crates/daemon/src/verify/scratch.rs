//! The scratch side of a verify run: the detached git worktree, its removal, and finding the processes a run left behind.
use capstan_operator::service::process_start_time;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs git in `cwd` with a clean environment: nothing of the daemon's own, no user or system configuration, no hooks,
/// no prompts. Ok carries the standard output; Err a one-line message.
pub fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git could not be run: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let mut message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if message.is_empty() {
        message = format!("git {} failed", args.first().copied().unwrap_or(""));
    }
    Err(message.lines().last().unwrap_or("").to_string())
}

/// `git worktree add --detach <path> <commit>` from the project's repository.
pub fn add_worktree(project_root: &Path, path: &Path, commit: &str) -> Result<(), String> {
    git(
        project_root,
        &[
            "worktree",
            "add",
            "--detach",
            "--force",
            &path.to_string_lossy(),
            commit,
        ],
    )
    .map(|_| ())
}

/// Removes a scratch directory and its git worktree record; best effort, never fails the caller.
pub fn remove_worktree(project_root: &Path, path: &Path) {
    if path.exists() {
        let _ = git(
            project_root,
            &["worktree", "remove", "--force", &path.to_string_lossy()],
        );
        force_remove_dir(path);
    }
    let _ = git(project_root, &["worktree", "prune"]);
}

/// `remove_dir_all`, after making directories writable once when the first attempt fails (a read-only module cache).
pub fn force_remove_dir(path: &Path) {
    if std::fs::remove_dir_all(path).is_ok() || !path.exists() {
        return;
    }
    make_writable(path);
    let _ = std::fs::remove_dir_all(path);
}

fn make_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    }
}

/// The environment entry that marks every process of one run.
pub fn marker(run_id: &str) -> String {
    format!("CAPSTAN_VERIFY_RUN_ID={run_id}")
}

/// Pids whose environment carries `marker` and that started at or after `min_start` (as `process_start_time` reads
/// them); never this process. Processes of other users cannot be read and are not found, nor, on macOS, Apple's own
/// binaries, whose environment the system hides (DEC-007).
pub fn marked_pids(marker: &str, min_start: Option<u64>) -> Vec<i32> {
    let me = std::process::id() as i32;
    let mut found = Vec::new();
    for pid in capstan_process::pids() {
        if pid == me || pid <= 1 {
            continue;
        }
        if !capstan_process::has_environment_entry(pid, marker) {
            continue;
        }
        if let Some(min) = min_start {
            let started = process_start_time(i64::from(pid)).and_then(|t| t.parse::<u64>().ok());
            if started.is_none_or(|started| started < min) {
                continue;
            }
        }
        found.push(pid);
    }
    found
}

/// Kills the marked processes of a run and waits for them to go (about two seconds at most). Returns how many it found.
pub fn reap_marked(marker: &str, min_start: Option<u64>) -> usize {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut total = 0;
    loop {
        let pids = marked_pids(marker, min_start);
        if pids.is_empty() {
            return total;
        }
        total = total.max(pids.len());
        for pid in pids {
            // SAFETY: a plain signal to a process that carries this run's marker.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        if Instant::now() >= deadline {
            return total;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Kills a process group the daemon started (SIGKILL).
pub fn kill_group(pgid: i64) {
    if pgid > 1 {
        // SAFETY: a plain signal to the process group of a child this daemon started.
        unsafe { libc::kill(-(pgid as libc::pid_t), libc::SIGKILL) };
    }
}

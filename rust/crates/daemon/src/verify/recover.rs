//! What a stopped or killed daemon left: runs still marked running, their process groups, and scratch directories.
use super::scratch;
use super::VerifyService;
use capstan_operator::service::ProcessOperations;
use serde_json::{json, Value};
use std::path::PathBuf;

impl VerifyService {
    /// Turns the runs left running into interrupted, kills the groups they left (only when the recorded leader is still the
    /// same process), cleans their scratch, removes stray scratch directories and trims the logs. Queued runs stay queued.
    pub fn recover(&self) {
        if let Err(problem) = self.prepare_scratch() {
            self.detail_log("verify_scratch_refused", json!({"error": problem}));
            *self.problem.lock().unwrap_or_else(|p| p.into_inner()) = Some(problem);
            return;
        }
        let interrupted =
            match self.step(|core, context| core.interrupt_active_verify_runs(context)) {
                Ok(answer) => answer["runs"].as_array().cloned().unwrap_or_default(),
                Err(error) => {
                    self.detail_log("verify_recover_failed", json!({"error": error.to_string()}));
                    return;
                }
            };
        for run in &interrupted {
            self.recover_run(run);
        }
        self.remove_strays();
        self.trim_logs();
    }

    fn recover_run(&self, run: &Value) {
        let run_id = run["runId"].as_str().unwrap_or("").to_string();
        let mut killed = false;
        let pgid = run["pgid"].as_i64();
        let leader_start = run["leaderStart"].as_str();
        if let (Some(pgid), Some(leader_start)) = (pgid, leader_start) {
            if self.processes.start_time(pgid).as_deref() == Some(leader_start) {
                self.processes.kill_group(pgid);
                killed = true;
            }
        }
        // Always by marker, even with reap = false: the ledger holds only the first step's group (the process record is
        // write-once), so whatever a later step started can only be found this way. `reap = false` protects tool daemons
        // of a live run; the daemon that ran this one is gone and nothing is waiting on them.
        let min_start = leader_start.and_then(|s| s.parse::<u64>().ok());
        killed |= scratch::reap_marked(&scratch::marker(&run_id), min_start) > 0;
        let commit = {
            let id = run_id.clone();
            self.kernel
                .run(move |core| core.verify_run(&id))
                .ok()
                .and_then(|r| r["commitSha"].as_str().map(str::to_string))
                .unwrap_or_default()
        };
        let path = run["scratchPath"].as_str().map(PathBuf::from);
        if let Some(path) = &path {
            // The setup may have run in it: the teardown hook is due as for any removed worktree.
            if path.exists() {
                self.teardown(&run_id, &commit, path, None);
            }
            scratch::remove_worktree(&self.project_root, path);
        }
        if pgid.is_some() {
            let input = json!({"runId": run_id});
            if let Err(error) =
                self.step(move |core, context| core.clear_verify_orphan(context, &input))
            {
                self.detail_log(
                    "verify_recover_failed",
                    json!({"runId": run_id, "error": error.to_string()}),
                );
            }
        }
        self.detail_log(
            "verify_run_recovered",
            json!({"runId": run_id, "killed": killed}),
        );
    }

    /// The scratch directory is ours or new: it is created with a marker file naming the project, or it already has that
    /// marker. A directory that holds other things and no marker is refused, so a misplaced `scratch_dir` (a folder of
    /// checkouts) is never cleaned.
    fn prepare_scratch(&self) -> Result<(), String> {
        use std::os::unix::fs::DirBuilderExt;
        let root = &self.scratch_root;
        let marker = root.join(SCRATCH_MARKER);
        let refuse = |why: &str| {
            Err(format!(
                "[verify] scratch_dir {} {why}; point scratch_dir at a new or empty directory",
                root.display()
            ))
        };
        match std::fs::read_to_string(&marker) {
            Ok(text) if text.trim() == self.project_id => return Ok(()),
            Ok(_) => return refuse("belongs to another project"),
            Err(_) => {}
        }
        match std::fs::read_dir(root) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return refuse("is not empty and was not made by capstan verify (no .capstan-verify marker)");
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(root)
                    .map_err(|e| {
                        format!(
                            "[verify] scratch_dir {} cannot be created: {e}",
                            root.display()
                        )
                    })?;
            }
            Err(error) => return refuse(&format!("cannot be read ({error})")),
        }
        std::fs::write(&marker, format!("{}\n", self.project_id)).map_err(|e| {
            format!(
                "[verify] scratch_dir {} cannot be written: {e}",
                root.display()
            )
        })
    }

    /// Whether `path` is a worktree this project's repository made: a `.git` file pointing into its `worktrees` directory.
    fn is_our_worktree(&self, path: &std::path::Path) -> bool {
        let Ok(text) = std::fs::read_to_string(path.join(".git")) else {
            return false;
        };
        let Some(target) = text.trim().strip_prefix("gitdir: ") else {
            return false;
        };
        match (
            std::fs::canonicalize(target),
            std::fs::canonicalize(self.project_root.join(".git").join("worktrees")),
        ) {
            (Ok(target), Ok(ours)) => target.starts_with(ours),
            _ => false,
        }
    }

    /// Leftover worktrees of this project's repository in the scratch root: a crash between `git worktree add` and the
    /// ledger step. Only an entry named like a run id whose `.git` file points into this repository is touched.
    fn remove_strays(&self) {
        let Ok(entries) = std::fs::read_dir(&self.scratch_root) else {
            return;
        };
        let mut removed = false;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let like_id = name.len() >= 8
                && name.len() <= 64
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            if !like_id || !entry.path().is_dir() || !self.is_our_worktree(&entry.path()) {
                continue;
            }
            scratch::remove_worktree(&self.project_root, &entry.path());
            removed = true;
        }
        if removed {
            let _ = scratch::git(&self.project_root, &["worktree", "prune"]);
        }
    }
}

/// The file that marks a directory as the scratch root of a project.
pub const SCRATCH_MARKER: &str = ".capstan-verify";

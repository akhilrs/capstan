//! Starts, restarts and re-adopts the agents that live in Herdr panes: the PM, workers spawned into their own git
//! worktrees, and the fallback watch pane (src/launcher.ts). It runs inside the daemon, which owns the adapter, and does
//! one operation at a time (the next one waits, a third is refused as busy). Every ledger write is its own closure on the
//! kernel thread; Herdr, git and the setup commands run on the calling thread, never inside one.
//!
//! The work lives in the sibling modules as methods of `LauncherKernel` (hub and PM, spawn, release and replace, setup and
//! teardown, observe and adopt); `Launcher` is the `LauncherService` the rest of the daemon holds.

use crate::api::{
    AnswerPromptRequest, BranchRenameResult, LaunchResult, LauncherResult, LauncherService,
    LauncherStatus, ObserveResult, ReleaseResult, ReplaceResult, SetupOutcome, SpawnOptions,
    SpawnResult, CLEANUP_BUDGET_MS,
};
use crate::kernel::LauncherKernel;
use crate::shared::LauncherOptions;
use capstan_herdr::api::{CaptureOutcome, RelayOutcome};
use std::collections::BTreeMap;

pub struct Launcher {
    kernel: LauncherKernel,
}

impl Launcher {
    pub fn new(options: LauncherOptions) -> Self {
        Self {
            kernel: LauncherKernel::new(options),
        }
    }

    /// The shared kernel of the launcher, for tests that look at its state.
    pub fn kernel(&self) -> &LauncherKernel {
        &self.kernel
    }
}

impl LauncherService for Launcher {
    fn launch_pm(&self) -> LauncherResult<LaunchResult> {
        Ok(self.kernel.launch_pm()?)
    }

    fn restart_pm(&self) -> LauncherResult<LaunchResult> {
        Ok(self.kernel.restart_pm()?)
    }

    fn spawn(&self, role_name: &str, options: &SpawnOptions) -> LauncherResult<SpawnResult> {
        Ok(self.kernel.spawn_worker(role_name, options)?)
    }

    fn rename_branch_for_task(
        &self,
        agent_id: &str,
        task: &str,
    ) -> LauncherResult<BranchRenameResult> {
        Ok(self.kernel.rename_branch_for_task(agent_id, task)?)
    }

    fn release(&self, agent_id: &str) -> LauncherResult<ReleaseResult> {
        Ok(self.kernel.release(agent_id)?)
    }

    fn replace(&self, agent_id: &str) -> LauncherResult<ReplaceResult> {
        Ok(self.kernel.replace(agent_id)?)
    }

    fn observe(&self, agent_id: &str, lines: usize) -> LauncherResult<ObserveResult> {
        Ok(self.kernel.observe_agent(agent_id, lines)?)
    }

    fn capture_prompt(&self, agent_id: &str) -> LauncherResult<CaptureOutcome> {
        Ok(self.kernel.capture_prompt(agent_id)?)
    }

    fn answer_prompt(
        &self,
        agent_id: &str,
        request: AnswerPromptRequest<'_>,
    ) -> LauncherResult<RelayOutcome> {
        Ok(self.kernel.answer_prompt(agent_id, request)?)
    }

    fn interrupt(&self, agent_id: &str) -> LauncherResult<bool> {
        Ok(self.kernel.interrupt(agent_id)?)
    }

    fn status(&self) -> LauncherStatus {
        self.kernel.launcher_status().unwrap_or_default()
    }

    fn operator_environment(&self) -> BTreeMap<String, String> {
        self.kernel.operator_environment()
    }

    fn in_flight_operations(&self) -> usize {
        self.kernel.in_flight()
    }

    fn adopt_all(&self) -> LauncherResult<()> {
        Ok(self.kernel.adopt_all()?)
    }

    fn teardown(&self, agent_id: &str, worktree_path: &str) -> LauncherResult<SetupOutcome> {
        let budget = self.kernel.budget(CLEANUP_BUDGET_MS);
        Ok(self
            .kernel
            .teardown_worktree(agent_id, worktree_path, &budget))
    }
}

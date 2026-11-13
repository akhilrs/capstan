//! The public surface of the controller core: one Rust method per public `ControllerCore` method of
//! src/controller/core.ts (same name in snake_case), delegating to `areas/<area>.rs`.
//!
//! Structured inputs and outputs travel as `serde_json::Value` in the camelCase shapes of the TypeScript records, so
//! the parity sequences compare them as JSON. The area packages fill the bodies in their area files; nothing here
//! changes when they do.

use crate::areas::{
    actors, agents, approval_windows, design_approvals, findings, idle_release, integrations,
    links, message_notices, messages, model_overrides, operator_grants, operator_proposals,
    operator_runs, panes, pauses, plan_archive, plan_landing, plan_packages, plan_status, plans,
    pm_stuck, prompt_relay, remote_decisions, reports, reviews, status, user_messages, verify_runs,
    warm_reviewers, workspace,
};
use crate::dispatch::{ErrorHandler, RestartFilter};
use crate::env::Env;
use crate::errors::KernelResult;
use crate::kernel::{Kernel, KernelOptions};
use crate::types::{InitialProject, MutationContext};
use serde_json::Value;
use std::path::Path;

/// `ControllerCore`.
pub struct Core {
    kernel: Kernel,
    /// When this core first ran the stuck-PM tick: `no_progress` is judged from `N` minutes after it.
    pm_stuck_watch_start: std::cell::Cell<Option<i64>>,
}

impl Core {
    pub fn open(
        state_dir: &Path,
        project: &InitialProject,
        options: &KernelOptions,
        env: Box<dyn Env>,
    ) -> KernelResult<Core> {
        Ok(Core {
            kernel: Kernel::open(state_dir, project, options, env)?,
            pm_stuck_watch_start: std::cell::Cell::new(None),
        })
    }

    /// A second connection without the project lock, for tests that race two writers on one ledger file.
    #[doc(hidden)]
    pub fn open_shared(
        state_dir: &Path,
        project: &InitialProject,
        options: &KernelOptions,
        env: Box<dyn Env>,
    ) -> KernelResult<Core> {
        Ok(Core {
            kernel: Kernel::open_shared(state_dir, project, options, env)?,
            pm_stuck_watch_start: std::cell::Cell::new(None),
        })
    }

    pub fn open_read_only(
        state_dir: &Path,
        project: &InitialProject,
        options: &KernelOptions,
        env: Box<dyn Env>,
    ) -> KernelResult<Core> {
        Ok(Core {
            kernel: Kernel::open_read_only(state_dir, project, options, env)?,
            pm_stuck_watch_start: std::cell::Cell::new(None),
        })
    }

    /// The kernel this core runs on.
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    /// Runs an operation by its `ControllerCore` method name with its arguments as JSON (what a parity sequence does).
    pub fn dispatch(&self, op: &str, args: &[Value]) -> KernelResult<Value> {
        match op {
            "projectId" => Ok(Value::String(self.project_id().to_string())),
            "stateVersion" => self.state_version().map(Value::from),
            "inputRevision" => self.input_revision().map(Value::from),
            "close" => {
                self.close();
                Ok(Value::Null)
            }
            _ => crate::dispatch::dispatch(&self.kernel, op, args),
        }
    }

    /// Statements the kernel connection has run: calls, not rows stepped.
    pub fn statements_run(&self) -> u64 {
        self.kernel.statements_run()
    }

    /// Calls of `Kernel::mutate` so far: calls, not rows written.
    pub fn mutate_calls(&self) -> u64 {
        self.kernel.mutate_calls()
    }

    pub fn project_id(&self) -> &str {
        &self.kernel.project_id
    }

    pub fn state_version(&self) -> KernelResult<i64> {
        self.kernel.state_version()
    }

    pub fn input_revision(&self) -> KernelResult<i64> {
        self.kernel.input_revision()
    }

    pub fn close(&self) {
        self.kernel.close();
    }

    /// `ControllerCore.createActor`.
    pub fn create_actor(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        actors::create_actor(&self.kernel, context, input)
    }

    /// `ControllerCore.revokeActor`.
    pub fn revoke_actor(&self, context: &MutationContext, actor_id: &str) -> KernelResult<Value> {
        actors::revoke_actor(&self.kernel, context, actor_id)
    }

    /// `ControllerCore.createSeat`.
    pub fn create_seat(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        actors::create_seat(&self.kernel, context, input)
    }

    /// `ControllerCore.roleDefinitions`.
    pub fn role_definitions(&self) -> KernelResult<Value> {
        actors::role_definitions(&self.kernel)
    }

    /// `ControllerCore.roleKind`.
    pub fn role_kind(&self, role_name: &str) -> KernelResult<Value> {
        actors::role_kind(&self.kernel, role_name)
    }

    /// `ControllerCore.syncRoleDefinitions`.
    pub fn sync_role_definitions(
        &self,
        context: &MutationContext,
        desired: &Value,
    ) -> KernelResult<Value> {
        actors::sync_role_definitions(&self.kernel, context, desired)
    }

    /// `ControllerCore.registerAgent`.
    pub fn register_agent(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        agents::register_agent(&self.kernel, context, input)
    }

    /// `ControllerCore.identify`.
    pub fn identify(&self, credential: &str) -> KernelResult<Value> {
        agents::identify(&self.kernel, credential)
    }

    /// `ControllerCore.listAgents`.
    pub fn list_agents(&self) -> KernelResult<Value> {
        agents::list_agents(&self.kernel)
    }

    /// `ControllerCore.activeAgents`.
    pub fn active_agents(&self) -> KernelResult<Value> {
        agents::active_agents(&self.kernel)
    }

    /// `ControllerCore.agentRecord`.
    pub fn agent_record(&self, agent_id: &str) -> KernelResult<Value> {
        agents::agent_record(&self.kernel, agent_id)
    }

    /// `ControllerCore.recordAgentObservation`.
    pub fn record_agent_observation(
        &self,
        context: &MutationContext,
        agent_id: &str,
        state: &Value,
    ) -> KernelResult<Value> {
        agents::record_agent_observation(&self.kernel, context, agent_id, state)
    }

    /// `ControllerCore.endAgent`.
    pub fn end_agent(
        &self,
        context: &MutationContext,
        agent_id: &str,
        options: Option<&Value>,
    ) -> KernelResult<Value> {
        agents::end_agent(&self.kernel, context, agent_id, options)
    }

    /// `ControllerCore.replaceAgentGeneration`.
    pub fn replace_agent_generation(
        &self,
        context: &MutationContext,
        agent_id: &str,
    ) -> KernelResult<Value> {
        agents::replace_agent_generation(&self.kernel, context, agent_id)
    }

    /// `ControllerCore.restartAgentGeneration`.
    pub fn restart_agent_generation(
        &self,
        context: &MutationContext,
        agent_id: &str,
    ) -> KernelResult<Value> {
        agents::restart_agent_generation(&self.kernel, context, agent_id)
    }

    /// `ControllerCore.markPmRestartsConsumed`.
    pub fn mark_pm_restarts_consumed(
        &self,
        context: &MutationContext,
        agent_id: &str,
        up_to_sequence: i64,
    ) -> KernelResult<Value> {
        agents::mark_pm_restarts_consumed(&self.kernel, context, agent_id, up_to_sequence)
    }

    /// `ControllerCore.recordAgentPane`.
    pub fn record_agent_pane(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        panes::record_agent_pane(&self.kernel, context, input)
    }

    /// `ControllerCore.clearAgentPane`.
    pub fn clear_agent_pane(
        &self,
        context: &MutationContext,
        agent_id: &str,
    ) -> KernelResult<Value> {
        panes::clear_agent_pane(&self.kernel, context, agent_id)
    }

    /// `ControllerCore.recordFallbackPane`.
    pub fn record_fallback_pane(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        panes::record_fallback_pane(&self.kernel, context, input)
    }

    /// `ControllerCore.clearFallbackPane`.
    pub fn clear_fallback_pane(&self, context: &MutationContext) -> KernelResult<Value> {
        panes::clear_fallback_pane(&self.kernel, context)
    }

    /// `ControllerCore.recordOrphanPane`.
    pub fn record_orphan_pane(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        panes::record_orphan_pane(&self.kernel, context, input)
    }

    /// `ControllerCore.clearOrphanPane`.
    pub fn clear_orphan_pane(
        &self,
        context: &MutationContext,
        pane_id: &str,
    ) -> KernelResult<Value> {
        panes::clear_orphan_pane(&self.kernel, context, pane_id)
    }

    /// `ControllerCore.orphanPanes`.
    pub fn orphan_panes(&self, credential: &str) -> KernelResult<Value> {
        panes::orphan_panes(&self.kernel, credential)
    }

    /// `ControllerCore.orphanPaneTerminals`.
    pub fn orphan_pane_terminals(&self, credential: &str) -> KernelResult<Value> {
        panes::orphan_pane_terminals(&self.kernel, credential)
    }

    /// `ControllerCore.agentPanes`.
    pub fn agent_panes(&self, credential: &str) -> KernelResult<Value> {
        panes::agent_panes(&self.kernel, credential)
    }

    /// `ControllerCore.paneTerminalId`.
    pub fn pane_terminal_id(&self, credential: &str, agent_id: &str) -> KernelResult<Value> {
        panes::pane_terminal_id(&self.kernel, credential, agent_id)
    }

    /// `ControllerCore.recordAgentReport`.
    pub fn record_agent_report(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        reports::record_agent_report(&self.kernel, context, input)
    }

    /// `ControllerCore.acceptedReportFor`.
    pub fn accepted_report_for(
        &self,
        agent_id: &str,
        generation: i64,
        commit_sha: &str,
    ) -> KernelResult<Value> {
        reports::accepted_report_for(&self.kernel, agent_id, generation, commit_sha)
    }

    /// `ControllerCore.unannouncedReports`.
    pub fn unannounced_reports(&self, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
        reports::unannounced_reports(&self.kernel, credential, limit)
    }

    /// `ControllerCore.announceReport`.
    pub fn announce_report(
        &self,
        context: &MutationContext,
        report_id: &str,
    ) -> KernelResult<Value> {
        reports::announce_report(&self.kernel, context, report_id)
    }

    /// The base commit a report's evidence recorded.
    pub fn report_base_sha(&self, report_id: &str) -> KernelResult<Option<String>> {
        reports::report_base_sha(&self.kernel, report_id)
    }

    /// `ControllerCore.agentReports`.
    pub fn agent_reports(&self, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
        reports::agent_reports(&self.kernel, credential, limit)
    }

    /// `ControllerCore.checkReviewRequest`.
    pub fn check_review_request(
        &self,
        subject_id: &str,
        reviewer_role: &str,
    ) -> KernelResult<Value> {
        reviews::check_review_request(&self.kernel, subject_id, reviewer_role)
    }

    /// `ControllerCore.beginReview`.
    pub fn begin_review(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        reviews::begin_review(&self.kernel, context, input)
    }

    /// `ControllerCore.completeReview`.
    pub fn complete_review(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        reviews::complete_review(&self.kernel, context, input)
    }

    /// `ControllerCore.unannouncedReviews`.
    pub fn unannounced_reviews(&self, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
        reviews::unannounced_reviews(&self.kernel, credential, limit)
    }

    /// `ControllerCore.announceReview`.
    pub fn announce_review(
        &self,
        context: &MutationContext,
        review_id: &str,
    ) -> KernelResult<Value> {
        reviews::announce_review(&self.kernel, context, review_id)
    }

    /// `ControllerCore.reviews`.
    pub fn reviews(&self, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
        reviews::reviews(&self.kernel, credential, limit)
    }

    /// `reviewShow`: a stored review for an identified caller.
    pub fn review_show(
        &self,
        review_id: &str,
        caller_agent_id: Option<&str>,
        privileged: bool,
    ) -> KernelResult<Value> {
        reviews::review_show(&self.kernel, review_id, caller_agent_id, privileged)
    }

    /// `ControllerCore.reviewsToRelease`.
    pub fn reviews_to_release(&self, credential: &str) -> KernelResult<Value> {
        reviews::reviews_to_release(&self.kernel, credential)
    }

    /// `ControllerCore.reviewPlanOf`.
    pub fn review_plan_of(&self, subject_id: &str) -> KernelResult<Value> {
        warm_reviewers::review_plan_of(&self.kernel, subject_id)
    }

    /// `ControllerCore.warmReviewerFor`.
    pub fn warm_reviewer_for(
        &self,
        subject_id: &str,
        reviewer_role: &str,
        max_reviews: i64,
    ) -> KernelResult<Value> {
        warm_reviewers::warm_reviewer_for(&self.kernel, subject_id, reviewer_role, max_reviews)
    }

    /// `ControllerCore.releaseWarmReviewer`.
    pub fn release_warm_reviewer(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        warm_reviewers::release_warm_reviewer(&self.kernel, context, input)
    }

    /// `ControllerCore.recordDesignDecision`.
    pub fn record_design_decision(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        design_approvals::record_design_decision(&self.kernel, context, input)
    }

    /// `ControllerCore.designApprovals`.
    pub fn design_approvals(&self, plan_id: &str, package_id: Option<&str>) -> KernelResult<Value> {
        design_approvals::design_approvals(&self.kernel, plan_id, package_id)
    }

    /// `ControllerCore.warmReviewers`.
    pub fn warm_reviewers(&self, credential: &str) -> KernelResult<Value> {
        warm_reviewers::warm_reviewers(&self.kernel, credential)
    }

    /// `ControllerCore.queueVerifyRun`.
    pub fn queue_verify_run(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        verify_runs::queue_verify_run(&self.kernel, context, input)
    }

    /// `ControllerCore.claimVerifyRun`.
    pub fn claim_verify_run(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        verify_runs::claim_verify_run(&self.kernel, context, input)
    }

    /// `ControllerCore.recordVerifyProcess`.
    pub fn record_verify_process(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        verify_runs::record_verify_process(&self.kernel, context, input)
    }

    /// `ControllerCore.finishVerifyRun`.
    pub fn finish_verify_run(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        verify_runs::finish_verify_run(&self.kernel, context, input)
    }

    /// `ControllerCore.cancelVerifyRun`.
    pub fn cancel_verify_run(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        verify_runs::cancel_verify_run(&self.kernel, context, input)
    }

    /// `ControllerCore.interruptActiveVerifyRuns`.
    pub fn interrupt_active_verify_runs(&self, context: &MutationContext) -> KernelResult<Value> {
        verify_runs::interrupt_active_verify_runs(&self.kernel, context)
    }

    /// `ControllerCore.clearVerifyOrphan`.
    pub fn clear_verify_orphan(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        verify_runs::clear_verify_orphan(&self.kernel, context, input)
    }

    /// `ControllerCore.verifyRun`.
    pub fn verify_run(&self, run_id: &str) -> KernelResult<Value> {
        verify_runs::verify_run(&self.kernel, run_id)
    }

    /// `ControllerCore.verifyRuns`.
    pub fn verify_runs(&self, subject_id: Option<&str>, limit: i64) -> KernelResult<Value> {
        verify_runs::verify_runs(&self.kernel, subject_id, limit)
    }

    /// `ControllerCore.nextQueuedVerifyRuns`.
    pub fn next_queued_verify_runs(&self, limit: i64) -> KernelResult<Value> {
        verify_runs::next_queued_verify_runs(&self.kernel, limit)
    }

    /// `ControllerCore.verifyGate`.
    pub fn verify_gate(&self, commit_sha: &str, commands_sha: &str) -> KernelResult<Value> {
        verify_runs::verify_gate(&self.kernel, "", commit_sha, commands_sha)
    }

    /// `ControllerCore.verifyGate` for one repo of a workspace ('' is the root repo).
    pub fn verify_gate_in_repo(
        &self,
        repo: &str,
        commit_sha: &str,
        commands_sha: &str,
    ) -> KernelResult<Value> {
        verify_runs::verify_gate(&self.kernel, repo, commit_sha, commands_sha)
    }

    /// `ControllerCore.verifyGateForSubjects`: the gate for the commit of each report or integration id.
    pub fn verify_gate_for_subjects(
        &self,
        subject_ids: &[String],
        commands_sha: &str,
    ) -> KernelResult<Value> {
        verify_runs::verify_gate_for_subjects(&self.kernel, subject_ids, commands_sha)
    }

    /// The review gate of a workspace subject, read from the ledger (see `reviews::review_gate`).
    pub fn review_gate(&self, subject: &crate::records::GateSubject) -> Result<(), String> {
        reviews::review_gate(&self.kernel, subject)
    }

    /// The runs that decide a gate subject, newest first, as `verify show` prints them.
    pub fn subject_runs(&self, subject: &crate::records::GateSubject) -> KernelResult<Vec<Value>> {
        verify_runs::subject_runs(&self.kernel, subject)
    }

    /// `ControllerCore.verifyStatus`.
    pub fn verify_status(&self) -> KernelResult<Value> {
        verify_runs::verify_status(&self.kernel)
    }

    /// `ControllerCore.beginIntegration`.
    pub fn begin_integration(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        integrations::begin_integration(&self.kernel, context, input)
    }

    /// `ControllerCore.finishIntegration`.
    pub fn finish_integration(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        integrations::finish_integration(&self.kernel, context, input)
    }

    /// `ControllerCore.settleIntegration`.
    pub fn settle_integration(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        integrations::settle_integration(&self.kernel, context, input)
    }

    /// `ControllerCore.openPlan`.
    pub fn open_plan(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plans::open_plan(&self.kernel, context, input)
    }

    /// `ControllerCore.submitPlan`.
    pub fn submit_plan(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plans::submit_plan(&self.kernel, context, input)
    }

    /// Lets a plan review answer findings in free text (the frozen Node corpus predates the finding line format).
    pub fn allow_free_text_plan_findings(&self, allowed: bool) {
        self.kernel.free_text_plan_findings.set(allowed);
    }

    /// `ControllerCore.amendPlan`.
    pub fn amend_plan(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plans::amend_plan(&self.kernel, context, input)
    }

    /// `ControllerCore.unannouncedPlanNotices`.
    pub fn unannounced_plan_notices(&self, credential: &str) -> KernelResult<Value> {
        plans::unannounced_plan_notices(&self.kernel, credential)
    }

    /// `ControllerCore.announcePlanNotice`.
    pub fn announce_plan_notice(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        plans::announce_plan_notice(&self.kernel, context, input)
    }

    /// `ControllerCore.planReviewRounds`.
    pub fn plan_review_rounds(&self, credential: &str, plan_id: &str) -> KernelResult<Value> {
        plans::plan_review_rounds(&self.kernel, credential, plan_id)
    }

    /// `ControllerCore.planReviewStatus`.
    pub fn plan_review_status(&self, credential: &str, plan_id: &str) -> KernelResult<Value> {
        plans::plan_review_status(&self.kernel, credential, plan_id)
    }

    /// `ControllerCore.abandonPlanReview`.
    pub fn abandon_plan_review(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        plans::abandon_plan_review(&self.kernel, context, input)
    }

    /// `ControllerCore.planRecord`.
    pub fn plan_record(&self, credential: &str, plan_id: &str) -> KernelResult<Value> {
        plans::plan_record(&self.kernel, credential, plan_id)
    }

    /// `ControllerCore.listPlans`.
    pub fn list_plans(&self, credential: &str) -> KernelResult<Value> {
        plans::list_plans(&self.kernel, credential)
    }

    /// `ControllerCore.assignPackage`.
    pub fn assign_package(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plan_packages::assign_package(&self.kernel, context, input)
    }

    /// `ControllerCore.recordSignoff`.
    pub fn record_signoff(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plan_packages::record_signoff(&self.kernel, context, input)
    }

    /// `ControllerCore.recordPackageLanding`.
    pub fn record_package_landing(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        plan_landing::record_package_landing(&self.kernel, context, input)
    }

    /// `ControllerCore.clearPackageLanding`.
    pub fn clear_package_landing(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        plan_landing::clear_package_landing(&self.kernel, context, input)
    }

    /// `ControllerCore.planStatuses`: the status of every plan, with the reasons a plan is stale when `withStale`.
    pub fn plan_statuses(&self, credential: &str, input: &Value) -> KernelResult<Value> {
        plan_status::plan_statuses(&self.kernel, credential, input)
    }

    /// `ControllerCore.linkExternal`.
    pub fn link_external(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        links::link_external(&self.kernel, context, input)
    }

    /// `ControllerCore.bindRequirement`.
    pub fn bind_requirement(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        links::bind_requirement(&self.kernel, context, input)
    }

    /// `ControllerCore.setModelOverride`.
    pub fn set_model_override(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        model_overrides::set_model_override(&self.kernel, context, input)
    }

    /// `ControllerCore.clearModelOverride`.
    pub fn clear_model_override(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        model_overrides::clear_model_override(&self.kernel, context, input)
    }

    /// `ControllerCore.modelOverrides`.
    pub fn model_overrides(&self, scope_type: &str, scope_id: &str) -> KernelResult<Value> {
        model_overrides::model_overrides(&self.kernel, scope_type, scope_id)
    }

    /// `ControllerCore.modelOverrideFor`.
    pub fn model_override_for(
        &self,
        scopes: &[(&str, &str)],
        kind: &str,
    ) -> KernelResult<Option<Value>> {
        model_overrides::model_override_for(&self.kernel, scopes, kind)
    }

    /// `ControllerCore.modelScopeOfSubject`.
    pub fn model_scope_of_subject(
        &self,
        subject_id: &str,
    ) -> KernelResult<Option<(String, String)>> {
        model_overrides::model_scope_of_subject(&self.kernel, subject_id)
    }

    /// `ControllerCore.agentModel`.
    pub fn agent_model(&self, agent_id: &str) -> KernelResult<Option<Value>> {
        model_overrides::agent_model(&self.kernel, agent_id)
    }

    /// `ControllerCore.taskNaming`.
    pub fn task_naming(&self, credential: &str, reference: &str) -> KernelResult<Value> {
        links::task_naming(&self.kernel, credential, reference)
    }

    /// `ControllerCore.agentHasReports`.
    pub fn agent_has_reports(&self, agent_id: &str) -> KernelResult<Value> {
        links::agent_has_reports(&self.kernel, agent_id)
    }

    /// `ControllerCore.activeBranchHolder`.
    pub fn active_branch_holder(
        &self,
        branch: &str,
        except_agent_id: Option<&str>,
    ) -> KernelResult<Value> {
        links::active_branch_holder(&self.kernel, branch, except_agent_id)
    }

    /// `ControllerCore.renameAgentBranch`.
    pub fn rename_agent_branch(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        links::rename_agent_branch(&self.kernel, context, input)
    }

    /// `ControllerCore.externalLinks`.
    pub fn external_links(&self, credential: &str) -> KernelResult<Value> {
        links::external_links(&self.kernel, credential)
    }

    /// The links of one plan and its packages, or of every plan: what `plan show` prints.
    pub fn plan_links(&self, credential: &str, plan_id: Option<&str>) -> KernelResult<Value> {
        links::plan_links(&self.kernel, credential, plan_id)
    }

    /// `ControllerCore.wantedNexoraState`.
    pub fn wanted_nexora_state(
        &self,
        credential: &str,
        ref_kind: &Value,
        ref_id: &str,
    ) -> KernelResult<Value> {
        links::wanted_nexora_state(&self.kernel, credential, ref_kind, ref_id)
    }

    /// `ControllerCore.syncDrift`.
    pub fn sync_drift(&self, credential: &str) -> KernelResult<Value> {
        links::sync_drift(&self.kernel, credential)
    }

    /// `ControllerCore.approvalNoticeNote`.
    pub fn approval_notice_note(&self, plan_id: &str) -> KernelResult<Value> {
        plans::approval_notice_note(&self.kernel, plan_id)
    }

    /// `ControllerCore.cancelPlan`.
    pub fn cancel_plan(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plan_packages::cancel_plan(&self.kernel, context, input)
    }

    /// Archives plans (all or nothing); operator only.
    pub fn archive_plans(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plan_archive::archive_plans(&self.kernel, context, input)
    }

    /// Unarchives one plan; operator only.
    pub fn unarchive_plan(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plan_archive::unarchive_plan(&self.kernel, context, input)
    }

    /// `ControllerCore.reassignPlan`.
    pub fn reassign_plan(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        plans::reassign_plan(&self.kernel, context, input)
    }

    /// `ControllerCore.integration`.
    pub fn integration(&self, integration_id: &str) -> KernelResult<Value> {
        integrations::integration(&self.kernel, integration_id)
    }

    /// `ControllerCore.integrationCommitInfo`.
    pub fn integration_commit_info(&self, integration_id: &str) -> KernelResult<Value> {
        integrations::integration_commit_info(&self.kernel, integration_id)
    }

    /// The details recorded when the integration was merged (`details-feed`); `None` when none were.
    pub fn integration_details(&self, integration_id: &str) -> KernelResult<Option<Value>> {
        integrations::integration_details(&self.kernel, integration_id)
    }

    /// `ControllerCore.plannedCommitInfo`.
    pub fn planned_commit_info(&self, report_ids: &Value) -> KernelResult<Value> {
        integrations::planned_commit_info(&self.kernel, report_ids)
    }

    /// `ControllerCore.integrationBranchRecorded`.
    pub fn integration_branch_recorded(&self, branch: &str) -> KernelResult<Value> {
        integrations::integration_branch_recorded(&self.kernel, branch)
    }

    /// `ControllerCore.integrations`.
    pub fn integrations(&self, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
        integrations::integrations(&self.kernel, credential, limit)
    }

    /// `ControllerCore.pipelineCounts`.
    pub fn pipeline_counts(&self, credential: &str) -> KernelResult<Value> {
        status::pipeline_counts(&self.kernel, credential)
    }

    /// `ControllerCore.awaitingConfirm`.
    pub fn awaiting_confirm(&self, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
        status::awaiting_confirm(&self.kernel, credential, limit)
    }

    /// `ControllerCore.settledIntegrations`.
    pub fn settled_integrations(
        &self,
        credential: &str,
        limit: Option<i64>,
    ) -> KernelResult<Value> {
        integrations::settled_integrations(&self.kernel, credential, limit)
    }

    /// `ControllerCore.coverageCandidates`.
    pub fn coverage_candidates(
        &self,
        credential: &str,
        integration_id: &str,
    ) -> KernelResult<Value> {
        integrations::coverage_candidates(&self.kernel, credential, integration_id)
    }

    /// `ControllerCore.recordCoveredReports`.
    pub fn record_covered_reports(
        &self,
        credential: &str,
        integration_id: &str,
        covered: &Value,
    ) -> KernelResult<Value> {
        integrations::record_covered_reports(&self.kernel, credential, integration_id, covered)
    }

    /// `ControllerCore.runningIntegrations`.
    pub fn running_integrations(&self, credential: &str) -> KernelResult<Value> {
        integrations::running_integrations(&self.kernel, credential)
    }

    /// `ControllerCore.recordAgentLost`.
    pub fn record_agent_lost(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        actors::record_agent_lost(&self.kernel, context, input)
    }

    /// `activeSuccessor`: the active agent that took over from an ended or replaced one, if any.
    pub fn active_successor(&self, agent_id: &str) -> KernelResult<Value> {
        actors::active_successor(&self.kernel, agent_id)
    }

    /// `replacementTip`: the last agent of the chain of recorded replacements from an agent.
    pub fn replacement_tip(&self, agent_id: &str) -> KernelResult<Value> {
        actors::replacement_tip(&self.kernel, agent_id)
    }

    /// `lostAgentIds`: the active agents recorded lost for their current generation.
    pub fn lost_agent_ids(&self) -> KernelResult<Value> {
        actors::lost_agent_ids(&self.kernel)
    }

    /// `ControllerCore.isAgentReplaced`.
    pub fn is_agent_replaced(&self, agent_id: &str) -> KernelResult<Value> {
        actors::is_agent_replaced(&self.kernel, agent_id)
    }

    /// `ControllerCore.recordAgentReplaced`.
    pub fn record_agent_replaced(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        actors::record_agent_replaced(&self.kernel, context, input)
    }

    /// `ControllerCore.activeTasks`.
    pub fn active_tasks(&self, credential: &str, caps: &Value) -> KernelResult<Value> {
        status::active_tasks(&self.kernel, credential, caps)
    }

    /// `ControllerCore.agentSeed`.
    pub fn agent_seed(&self, agent_id: &str) -> KernelResult<Value> {
        actors::agent_seed(&self.kernel, agent_id)
    }

    /// `ControllerCore.raiseFinding`.
    pub fn raise_finding(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        findings::raise_finding(&self.kernel, context, input)
    }

    /// `ControllerCore.checkFinding`.
    pub fn check_finding(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        findings::check_finding(&self.kernel, context, input)
    }

    /// `ControllerCore.unannouncedFindingNotices`.
    pub fn unannounced_finding_notices(
        &self,
        credential: &str,
        limit: Option<i64>,
    ) -> KernelResult<Value> {
        findings::unannounced_finding_notices(&self.kernel, credential, limit)
    }

    /// `ControllerCore.announceFindingNotice`.
    pub fn announce_finding_notice(
        &self,
        context: &MutationContext,
        notice_id: &str,
    ) -> KernelResult<Value> {
        findings::announce_finding_notice(&self.kernel, context, notice_id)
    }

    /// `ControllerCore.assertCanObserve`.
    pub fn assert_can_observe(&self, credential: &str) -> KernelResult<Value> {
        findings::assert_can_observe(&self.kernel, credential)
    }

    /// `ControllerCore.findings`.
    pub fn findings(&self, credential: &str, limit: Option<i64>) -> KernelResult<Value> {
        findings::findings(&self.kernel, credential, limit)
    }

    /// `ControllerCore.sweepFindings`.
    pub fn sweep_findings(
        &self,
        new_context: &dyn Fn() -> MutationContext,
        deadline_seconds: f64,
        on_error: Option<ErrorHandler<'_>>,
    ) -> KernelResult<Value> {
        findings::sweep_findings(&self.kernel, new_context, deadline_seconds, on_error)
    }

    /// `ControllerCore.fallbackPane`.
    pub fn fallback_pane(&self, credential: &str) -> KernelResult<Value> {
        panes::fallback_pane(&self.kernel, credential)
    }

    /// `ControllerCore.pmRestarts`.
    pub fn pm_restarts(&self, credential: &str, agent_id: &str) -> KernelResult<Value> {
        agents::pm_restarts(&self.kernel, credential, agent_id)
    }

    /// `ControllerCore.seatActorIds`.
    pub fn seat_actor_ids(&self, credential: &str, seat_id: &str) -> KernelResult<Value> {
        actors::seat_actor_ids(&self.kernel, credential, seat_id)
    }

    /// One remote decision (see `areas/remote_decisions.rs`): a single mutation that applies or forwards the verb and
    /// writes the row of its nonce.
    pub fn decide(
        &self,
        context: &MutationContext,
        input: &remote_decisions::DecideInput,
    ) -> KernelResult<Value> {
        remote_decisions::decide(self, context, input)
    }

    /// `ControllerCore.enqueueMessage`.
    pub fn enqueue_message(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        messages::enqueue_message(&self.kernel, context, input)
    }

    /// `user_message.send`: the calling agent sends the user a message (`cstan send @user`).
    pub fn send_to_user(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        user_messages::send_to_user(&self.kernel, context, input)
    }

    /// `user_message.close`: the asking agent marks its question answered in the terminal.
    pub fn close_user_question(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        user_messages::close_question(&self.kernel, context, input)
    }

    /// `message.requeue`: after a PM restart, the user's and agents' cancelled open messages are queued for the new PM.
    pub fn requeue_after_restart(
        &self,
        context: &MutationContext,
        agent_id: &str,
    ) -> KernelResult<Value> {
        messages::requeue_after_restart(&self.kernel, context, agent_id)
    }

    /// `ControllerCore.message`.
    pub fn message(&self, message_id: &str) -> KernelResult<Value> {
        messages::message(&self.kernel, message_id)
    }

    /// `ControllerCore.messagesFor`.
    pub fn messages_for(&self, agent_id: &str) -> KernelResult<Value> {
        messages::messages_for(&self.kernel, agent_id)
    }

    /// `ControllerCore.openMessagesFor`.
    pub fn open_messages_for(&self, agent_id: &str) -> KernelResult<Value> {
        messages::open_messages_for(&self.kernel, agent_id)
    }

    /// The states of the given messages and the latest send or ack time of the agent's mail, without bodies.
    pub fn pm_mail_digest(&self, agent_id: &str, message_ids: &[String]) -> KernelResult<Value> {
        messages::pm_mail_digest(&self.kernel, agent_id, message_ids)
    }

    /// `ControllerCore.unresolvedMessages`.
    pub fn unresolved_messages(&self, credential: &str, limit: i64) -> KernelResult<Value> {
        messages::unresolved_messages(&self.kernel, credential, limit)
    }

    /// `ControllerCore.inputClears`.
    pub fn input_clears(&self, credential: &str, limit: i64) -> KernelResult<Value> {
        messages::input_clears(&self.kernel, credential, limit)
    }

    /// `ControllerCore.openWaits`.
    pub fn open_waits(&self, credential: &str) -> KernelResult<Value> {
        message_notices::open_waits(&self.kernel, credential)
    }

    /// `ControllerCore.senderOf`.
    pub fn sender_of(&self, actor_id: &str) -> KernelResult<Value> {
        messages::sender_of(&self.kernel, actor_id)
    }

    /// `ControllerCore.messageRejections`.
    pub fn message_rejections(&self) -> KernelResult<Value> {
        messages::message_rejections(&self.kernel)
    }

    /// `ControllerCore.agentInbox`.
    pub fn agent_inbox(&self, credential: &str, agent_id: Option<&str>) -> KernelResult<Value> {
        messages::agent_inbox(&self.kernel, credential, agent_id)
    }

    /// `ControllerCore.pullMessage`.
    pub fn pull_message(&self, context: &MutationContext) -> KernelResult<Value> {
        messages::pull_message(&self.kernel, context)
    }

    /// `ControllerCore.pullPending`.
    pub fn pull_pending(&self, credential: &str) -> KernelResult<Value> {
        messages::pull_pending(&self.kernel, credential)
    }

    /// Sets the message policy (`pmWakeClass`, `infoWakeAfterSeconds`, `noticeClass`) read from the timers value; absent
    /// fields keep the legacy behaviour.
    pub fn set_message_policy(&self, timers: &Value) {
        *self.kernel.message_policy.borrow_mut() =
            crate::areas::messaging::MessagePolicy::from_value(timers);
    }

    /// `deliverInbox`: pulls everything pending; with `digest`, acks the delivered info messages at once.
    pub fn deliver_inbox(&self, credential: &str, digest: bool) -> KernelResult<Value> {
        messages::deliver_inbox(&self.kernel, credential, digest)
    }

    /// `wakePending`: read-only; whether a pending message of the caller wakes it under the policy.
    pub fn wake_pending(&self, credential: &str) -> KernelResult<Value> {
        messages::wake_pending(&self.kernel, credential)
    }

    /// `ackMessages`: validates every id first; one bad id refuses all.
    pub fn ack_messages(
        &self,
        context: &MutationContext,
        message_ids: &[String],
    ) -> KernelResult<Value> {
        messages::ack_messages(&self.kernel, context, message_ids)
    }

    /// `ackAllRead`: acks every sent or unacked message of the caller.
    pub fn ack_all_read(&self, context: &MutationContext) -> KernelResult<Value> {
        messages::ack_all_read(&self.kernel, context)
    }

    /// `ControllerCore.unreadSummary`.
    pub fn unread_summary(&self, credential: &str) -> KernelResult<Value> {
        messages::unread_summary(&self.kernel, credential)
    }

    /// `ControllerCore.recordDeferral`.
    pub fn record_deferral(
        &self,
        context: &MutationContext,
        message_id: &str,
        reason: &Value,
    ) -> KernelResult<Value> {
        messages::record_deferral(&self.kernel, context, message_id, reason)
    }

    /// `ControllerCore.recordInputClear`.
    pub fn record_input_clear(
        &self,
        context: &MutationContext,
        message_id: &str,
        text: &str,
    ) -> KernelResult<Value> {
        messages::record_input_clear(&self.kernel, context, message_id, text)
    }

    /// `ControllerCore.recordSent`.
    pub fn record_sent(&self, context: &MutationContext, message_id: &str) -> KernelResult<Value> {
        messages::record_sent(&self.kernel, context, message_id)
    }

    /// `ControllerCore.recordFailure`.
    pub fn record_failure(
        &self,
        context: &MutationContext,
        message_id: &str,
        reason: &str,
    ) -> KernelResult<Value> {
        messages::record_failure(&self.kernel, context, message_id, reason)
    }

    /// `ControllerCore.recordNotification`.
    pub fn record_notification(
        &self,
        context: &MutationContext,
        message_id: &str,
    ) -> KernelResult<Value> {
        message_notices::record_notification(&self.kernel, context, message_id)
    }

    /// `ControllerCore.ackMessage`.
    pub fn ack_message(&self, context: &MutationContext, message_id: &str) -> KernelResult<Value> {
        messages::ack_message(&self.kernel, context, message_id)
    }

    /// `ControllerCore.resolveMessage`.
    pub fn resolve_message(
        &self,
        context: &MutationContext,
        message_id: &str,
        decision: &Value,
        note: Option<&str>,
    ) -> KernelResult<Value> {
        messages::resolve_message(&self.kernel, context, message_id, decision, note)
    }

    /// `ControllerCore.beginWait`.
    pub fn begin_wait(&self, context: &MutationContext) -> KernelResult<Value> {
        message_notices::begin_wait(&self.kernel, context)
    }

    /// `ControllerCore.endWait`.
    pub fn end_wait(&self, context: &MutationContext, wait_id: &str) -> KernelResult<Value> {
        message_notices::end_wait(&self.kernel, context, wait_id)
    }

    /// `ControllerCore.endWaitAsController`.
    pub fn end_wait_as_controller(
        &self,
        context: &MutationContext,
        wait_id: &str,
    ) -> KernelResult<Value> {
        message_notices::end_wait_as_controller(&self.kernel, context, wait_id)
    }

    /// `ControllerCore.advanceMessaging`.
    pub fn advance_messaging(
        &self,
        context: &MutationContext,
        timers: &Value,
    ) -> KernelResult<Value> {
        message_notices::advance_messaging(&self.kernel, context, timers)
    }

    /// `ControllerCore.queueMissingDeliveryNotices`.
    pub fn queue_missing_delivery_notices(&self, context: &MutationContext) -> KernelResult<Value> {
        message_notices::queue_missing_delivery_notices(&self.kernel, context)
    }

    /// `ControllerCore.queueInputBlockedNotice`.
    pub fn queue_input_blocked_notice(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        message_notices::queue_input_blocked_notice(&self.kernel, context, input)
    }

    /// `ControllerCore.queueAttentionNotices`.
    pub fn queue_attention_notices(
        &self,
        context: &MutationContext,
        episodes: &Value,
    ) -> KernelResult<Value> {
        message_notices::queue_attention_notices(&self.kernel, context, episodes)
    }

    /// `ControllerCore.recordPmWake`.
    pub fn record_pm_wake(
        &self,
        context: &MutationContext,
        message_id: &str,
    ) -> KernelResult<Value> {
        message_notices::record_pm_wake(&self.kernel, context, message_id)
    }

    /// `ControllerCore.queueSupervisionCheck`.
    pub fn queue_supervision_check(
        &self,
        context: &MutationContext,
        interval_seconds: f64,
    ) -> KernelResult<Value> {
        message_notices::queue_supervision_check(&self.kernel, context, interval_seconds)
    }

    /// The stuck-PM tick (`controller:reconcile`): raises or closes at most one pass of episodes (see `pm_stuck`).
    /// `child_activity` is the last tool-process activity per agent id in milliseconds.
    pub fn pm_stuck_tick(
        &self,
        context: &MutationContext,
        child_activity: &std::collections::HashMap<String, u64>,
        now_ms: i64,
        minutes: i64,
    ) -> KernelResult<Value> {
        let watch_start = match self.pm_stuck_watch_start.get() {
            Some(start) => start,
            None => {
                self.pm_stuck_watch_start.set(Some(now_ms));
                now_ms
            }
        };
        pm_stuck::pm_stuck_tick(
            &self.kernel,
            context,
            child_activity,
            now_ms,
            minutes,
            watch_start,
        )
    }

    /// Why an agent must not be released automatically; empty when nothing holds it (see `idle_release`).
    pub fn open_work(&self, agent_id: &str) -> KernelResult<Vec<String>> {
        idle_release::open_work(&self.kernel, agent_id)
    }

    /// The workers idle by the ledger's evidence at `now_ms` (see `idle_release::idle_candidates` for `input`).
    pub fn idle_candidates(&self, now_ms: i64, input: &Value) -> KernelResult<Vec<Value>> {
        idle_release::idle_candidates(&self.kernel, now_ms, input)
    }

    /// The end of the newest keep record of an agent, or `None`.
    pub fn keep_until(&self, agent_id: &str) -> KernelResult<Option<String>> {
        idle_release::keep_until(&self.kernel, agent_id)
    }

    /// Records a keep of an agent from automatic release (`hours` 0 clears it).
    pub fn record_keep(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        idle_release::record_keep(&self.kernel, context, input)
    }

    /// The idle facts `cstan status` shows of every active worker (see `idle_release::idle_overview`).
    pub fn idle_overview(&self, now_ms: i64, input: &Value) -> KernelResult<Vec<Value>> {
        idle_release::idle_overview(&self.kernel, now_ms, input)
    }

    /// The newest idle warning of an agent: `{warnedAt, idleSince, releaseAt}`, or `None`.
    pub fn last_idle_warning(&self, agent_id: &str) -> KernelResult<Option<Value>> {
        idle_release::last_idle_warning(&self.kernel, agent_id)
    }

    /// Records the warning to the PM about an idle agent (`controller:reconcile`).
    pub fn record_idle_warning(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        idle_release::record_idle_warning(&self.kernel, context, input)
    }

    /// Records that an idle episode ended without a release (`controller:reconcile`).
    pub fn record_idle_episode_end(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        idle_release::record_idle_episode_end(&self.kernel, context, input)
    }

    /// Records an automatic release after the launcher ended the agent (`controller:reconcile`).
    pub fn record_auto_release(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        idle_release::record_auto_release(&self.kernel, context, input)
    }

    /// `ControllerCore.readiness`.
    pub fn readiness(&self, work_item_id: &str) -> KernelResult<Value> {
        status::readiness(&self.kernel, work_item_id)
    }

    /// `ControllerCore.transitionRun`.
    pub fn transition_run(
        &self,
        context: &MutationContext,
        to_state: &Value,
        reason: Option<&str>,
    ) -> KernelResult<Value> {
        pauses::transition_run(&self.kernel, context, to_state, reason)
    }

    /// `ControllerCore.pauseState`.
    pub fn pause_state(&self) -> KernelResult<Value> {
        pauses::pause_state(&self.kernel)
    }

    /// `ControllerCore.isDeliveryPaused`.
    pub fn is_delivery_paused(&self, agent_id: &str) -> KernelResult<Value> {
        pauses::is_delivery_paused(&self.kernel, agent_id)
    }

    /// `ControllerCore.assertRunNotPaused`.
    pub fn assert_run_not_paused(&self, action: &str) -> KernelResult<Value> {
        pauses::assert_run_not_paused(&self.kernel, action)
    }

    /// `ControllerCore.pauseAgent`.
    pub fn pause_agent(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        pauses::pause_agent(&self.kernel, context, input)
    }

    /// `ControllerCore.resumeAgent`.
    pub fn resume_agent(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        pauses::resume_agent(&self.kernel, context, input)
    }

    /// `ControllerCore.supervisionReason`.
    pub fn supervision_reason(&self, credential: &str) -> KernelResult<Value> {
        findings::supervision_reason(&self.kernel, credential)
    }

    /// `ControllerCore.supervisionActivity`.
    pub fn supervision_activity(&self) -> KernelResult<Value> {
        findings::supervision_activity(&self.kernel)
    }

    /// `ControllerCore.statusSnapshot`.
    pub fn status_snapshot(&self) -> KernelResult<Value> {
        status::status_snapshot(&self.kernel)
    }

    /// `ControllerCore.inspect`.
    pub fn inspect(&self, id: &str) -> KernelResult<Value> {
        status::inspect(&self.kernel, id)
    }

    /// `ControllerCore.activeOperatorGrants`.
    pub fn active_operator_grants(&self) -> KernelResult<Value> {
        operator_grants::active_operator_grants(&self.kernel)
    }

    /// `ControllerCore.listOperatorGrants`.
    pub fn list_operator_grants(&self, limit: Option<i64>) -> KernelResult<Value> {
        operator_grants::list_operator_grants(&self.kernel, limit)
    }

    /// `ControllerCore.operatorProposal`.
    pub fn operator_proposal(&self, proposal_id: &str) -> KernelResult<Value> {
        operator_proposals::operator_proposal(&self.kernel, proposal_id)
    }

    /// `ControllerCore.listOperatorProposals`.
    pub fn list_operator_proposals(&self, filter: Option<&Value>) -> KernelResult<Value> {
        operator_proposals::list_operator_proposals(&self.kernel, filter)
    }

    /// `ControllerCore.pendingOperatorProposalCount`.
    pub fn pending_operator_proposal_count(&self, agent_id: &str) -> KernelResult<Value> {
        operator_proposals::pending_operator_proposal_count(&self.kernel, agent_id)
    }

    /// `ControllerCore.approvedOperatorProposals`.
    pub fn approved_operator_proposals(&self) -> KernelResult<Value> {
        operator_proposals::approved_operator_proposals(&self.kernel)
    }

    /// `ControllerCore.runningOperatorProposal`.
    pub fn running_operator_proposal(&self) -> KernelResult<Value> {
        operator_proposals::running_operator_proposal(&self.kernel)
    }

    /// `ControllerCore.unclearedOperatorOrphans`.
    pub fn uncleared_operator_orphans(&self) -> KernelResult<Value> {
        operator_runs::uncleared_operator_orphans(&self.kernel)
    }

    /// `ControllerCore.busyIndicators`.
    pub fn busy_indicators(&self) -> KernelResult<Value> {
        status::busy_indicators(&self.kernel)
    }

    /// `ControllerCore.proposeOperatorAction`.
    pub fn propose_operator_action(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        operator_proposals::propose_operator_action(&self.kernel, context, input)
    }

    /// `ControllerCore.decideOperatorProposal`.
    pub fn decide_operator_proposal(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        operator_proposals::decide_operator_proposal(&self.kernel, context, input)
    }

    /// `ControllerCore.cancelOperatorProposal`.
    pub fn cancel_operator_proposal(
        &self,
        context: &MutationContext,
        proposal_id: &str,
    ) -> KernelResult<Value> {
        operator_proposals::cancel_operator_proposal(&self.kernel, context, proposal_id)
    }

    /// `ControllerCore.dueOperatorGrantExpiries`.
    pub fn due_operator_grant_expiries(&self, also_due: Option<&[String]>) -> KernelResult<Value> {
        operator_grants::due_operator_grant_expiries(&self.kernel, also_due)
    }

    /// `ControllerCore.expireOperatorGrants`.
    pub fn expire_operator_grants(
        &self,
        context: &MutationContext,
        also_due: Option<&[String]>,
    ) -> KernelResult<Value> {
        operator_grants::expire_operator_grants(&self.kernel, context, also_due)
    }

    /// `ControllerCore.endOperatorGrantsForRestart`.
    pub fn end_operator_grants_for_restart(
        &self,
        context: &MutationContext,
    ) -> KernelResult<Value> {
        operator_grants::end_operator_grants_for_restart(&self.kernel, context)
    }

    /// `ControllerCore.revokeOperatorGrant`.
    pub fn revoke_operator_grant(
        &self,
        context: &MutationContext,
        grant_id: &str,
    ) -> KernelResult<Value> {
        operator_grants::revoke_operator_grant(&self.kernel, context, grant_id)
    }

    /// The open approval window, or null.
    pub fn active_approval_window(&self) -> KernelResult<Value> {
        approval_windows::active_approval_window(&self.kernel)
    }

    /// Approval windows, newest first.
    pub fn list_approval_windows(&self, limit: Option<i64>) -> KernelResult<Value> {
        approval_windows::list_approval_windows(&self.kernel, limit)
    }

    /// Whether the open approval window is past its cap; read it before `expire_approval_windows`.
    pub fn approval_window_due(&self) -> KernelResult<Value> {
        approval_windows::approval_window_due(&self.kernel)
    }

    /// Ends the open approval window when it is past its cap.
    pub fn expire_approval_windows(&self, context: &MutationContext) -> KernelResult<Value> {
        approval_windows::expire_approval_windows(&self.kernel, context)
    }

    /// Ends the open approval window at startup.
    pub fn end_approval_windows_for_restart(
        &self,
        context: &MutationContext,
    ) -> KernelResult<Value> {
        approval_windows::end_approval_windows_for_restart(&self.kernel, context)
    }

    /// Revokes one approval window.
    pub fn end_approval_window(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        approval_windows::end_approval_window(&self.kernel, context, input)
    }

    /// Tells the PM once per relay that a prompt hit the approval floor.
    pub fn queue_floor_hit_notice(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        message_notices::queue_floor_hit_notice(&self.kernel, context, input)
    }

    /// `ControllerCore.recordOperatorFullAuto`.
    pub fn record_operator_full_auto(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        operator_grants::record_operator_full_auto(&self.kernel, context, input)
    }

    /// `ControllerCore.endFullAutoProposal`.
    pub fn end_full_auto_proposal(
        &self,
        context: &MutationContext,
        proposal_id: &str,
    ) -> KernelResult<Value> {
        operator_grants::end_full_auto_proposal(&self.kernel, context, proposal_id)
    }

    /// `ControllerCore.dueOperatorExpiries`.
    pub fn due_operator_expiries(&self, limits: &Value) -> KernelResult<Value> {
        operator_proposals::due_operator_expiries(&self.kernel, limits)
    }

    /// `ControllerCore.expireOperatorProposals`.
    pub fn expire_operator_proposals(
        &self,
        context: &MutationContext,
        limits: &Value,
    ) -> KernelResult<Value> {
        operator_proposals::expire_operator_proposals(&self.kernel, context, limits)
    }

    /// `ControllerCore.claimOperatorRun`.
    pub fn claim_operator_run(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        operator_runs::claim_operator_run(&self.kernel, context, input)
    }

    /// `ControllerCore.recordOperatorRunProcess`.
    pub fn record_operator_run_process(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        operator_runs::record_operator_run_process(&self.kernel, context, input)
    }

    /// `ControllerCore.finishOperatorRun`.
    pub fn finish_operator_run(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        operator_runs::finish_operator_run(&self.kernel, context, input)
    }

    /// `ControllerCore.abandonRunningOperatorRuns`.
    pub fn abandon_running_operator_runs(
        &self,
        context: &MutationContext,
        skip_restarts_with_plan: Option<RestartFilter<'_>>,
    ) -> KernelResult<Value> {
        operator_runs::abandon_running_operator_runs(&self.kernel, context, skip_restarts_with_plan)
    }

    /// `ControllerCore.clearOperatorOrphan`.
    pub fn clear_operator_orphan(
        &self,
        context: &MutationContext,
        proposal_id: &str,
    ) -> KernelResult<Value> {
        operator_runs::clear_operator_orphan(&self.kernel, context, proposal_id)
    }

    /// `ControllerCore.configurePromptRelay`.
    pub fn configure_prompt_relay(&self, config: &Value) -> KernelResult<Value> {
        prompt_relay::configure(&self.kernel, config)
    }

    /// `ControllerCore.promptRelayEnabled`.
    pub fn prompt_relay_enabled(&self) -> KernelResult<bool> {
        prompt_relay::enabled(&self.kernel)
    }

    /// `ControllerCore.checkPromptAnswer`.
    pub fn check_prompt_answer(
        &self,
        relay_id: &str,
        hash: &Value,
        answer: &Value,
    ) -> KernelResult<Value> {
        prompt_relay::check_prompt_answer(&self.kernel, relay_id, hash, answer)
    }

    /// `ControllerCore.promptRelay`.
    pub fn prompt_relay(&self, relay_id: &str) -> KernelResult<Value> {
        prompt_relay::prompt_relay(&self.kernel, relay_id)
    }

    /// `ControllerCore.listPromptRelays`.
    pub fn list_prompt_relays(&self, limit: Option<i64>) -> KernelResult<Value> {
        prompt_relay::list_prompt_relays(&self.kernel, limit)
    }

    /// `ControllerCore.promptRelayStatus`.
    pub fn prompt_relay_status(&self) -> KernelResult<Value> {
        prompt_relay::prompt_relay_status(&self.kernel)
    }

    /// `ControllerCore.recordPromptCapture`.
    pub fn record_prompt_capture(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        prompt_relay::record_prompt_capture(&self.kernel, context, input)
    }

    /// `ControllerCore.beginPromptAnswer`.
    pub fn begin_prompt_answer(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        prompt_relay::begin_prompt_answer(&self.kernel, context, input)
    }

    /// `ControllerCore.refusePromptAnswer`.
    pub fn refuse_prompt_answer(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        prompt_relay::refuse_prompt_answer(&self.kernel, context, input)
    }

    /// `ControllerCore.finishPromptAnswer`.
    pub fn finish_prompt_answer(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        prompt_relay::finish_prompt_answer(&self.kernel, context, input)
    }

    /// `ControllerCore.hasExpiredPromptCaptures`.
    pub fn has_expired_prompt_captures(&self) -> KernelResult<Value> {
        prompt_relay::has_expired_prompt_captures(&self.kernel)
    }

    /// `ControllerCore.expirePromptCaptures`.
    pub fn expire_prompt_captures(&self, context: &MutationContext) -> KernelResult<Value> {
        prompt_relay::expire_prompt_captures(&self.kernel, context)
    }

    /// `ControllerCore.failInterruptedPromptRelays`.
    pub fn fail_interrupted_prompt_relays(&self, context: &MutationContext) -> KernelResult<Value> {
        prompt_relay::fail_interrupted_prompt_relays(&self.kernel, context)
    }

    /// Records the worktree an agent holds in a repo (a removed one is revived; a live one is `already_added`).
    pub fn record_agent_worktree(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::record_agent_worktree(&self.kernel, context, input)
    }

    /// Marks an agent's live worktree in a repo as removed.
    pub fn remove_agent_worktree(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::remove_agent_worktree(&self.kernel, context, input)
    }

    /// Marks every live worktree of an agent's current generation as removed, in one transaction.
    pub fn remove_agent_worktrees(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::remove_agent_worktrees(&self.kernel, context, input)
    }

    /// The live worktrees of an agent's current generation, by repo.
    pub fn list_agent_worktrees(&self, agent_id: &str) -> KernelResult<Value> {
        workspace::list_agent_worktrees(&self.kernel, agent_id)
    }

    /// On a replace, copies the live worktrees of the old generation to the new one in one transaction.
    pub fn copy_agent_worktrees(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::copy_agent_worktrees(&self.kernel, context, input)
    }

    /// The repo an integration belongs to (it is a member of a set); `None` for the root repo.
    pub fn repo_of_integration(&self, integration_id: &str) -> KernelResult<Option<String>> {
        workspace::repo_of_integration(&self.kernel, integration_id)
    }

    /// Whether the project has any report commit or integration set row (false for a single-repo project).
    pub fn has_workspace_rows(&self) -> KernelResult<bool> {
        workspace::has_workspace_rows(&self.kernel)
    }

    /// The commits a report named, by repo.
    pub fn report_commits(&self, report_id: &str) -> KernelResult<Value> {
        workspace::report_commits(&self.kernel, report_id)
    }

    /// Creates a running integration set of the repos given.
    pub fn create_integration_set(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::create_integration_set(&self.kernel, context, input)
    }

    /// Moves an integration set to a state.
    pub fn set_integration_state(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::set_integration_state(&self.kernel, context, input)
    }

    /// Attaches the integration created for a repo of a set (the member's attempt grows by one).
    pub fn attach_set_member(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::attach_set_member(&self.kernel, context, input)
    }

    /// Lets go of a member's integration.
    pub fn detach_set_member(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::detach_set_member(&self.kernel, context, input)
    }

    /// One integration set with its members; null when there is none.
    pub fn integration_set(&self, set_id: &str) -> KernelResult<Value> {
        workspace::integration_set(&self.kernel, set_id)
    }

    /// The set an integration is a member of.
    pub fn set_of_integration(&self, integration_id: &str) -> KernelResult<Option<String>> {
        workspace::set_of_integration(&self.kernel, integration_id)
    }

    /// Records the repo a cleanup ran in.
    pub fn record_cleanup_repo(
        &self,
        context: &MutationContext,
        input: &Value,
    ) -> KernelResult<Value> {
        workspace::record_cleanup_repo(&self.kernel, context, input)
    }

    /// The repo of a cleanup; '' (the root) for an older cleanup.
    pub fn cleanup_repo(&self, cleanup_id: &str) -> KernelResult<String> {
        workspace::cleanup_repo(&self.kernel, cleanup_id)
    }

    /// The repos a package names in the approved revision of its plan; empty without repos.
    pub fn package_repos(&self, plan_id: &str, package_id: &str) -> KernelResult<Vec<String>> {
        plan_packages::package_repos(&self.kernel, plan_id, package_id)
    }

    /// Whether every member of a set has a merged integration.
    pub fn set_complete(&self, set_id: &str) -> KernelResult<bool> {
        workspace::set_complete(&self.kernel, set_id)
    }

    /// Whether every live member of a set is confirmed (a dropped member is skipped; a row-only member blocks).
    pub fn set_confirmed(&self, set_id: &str) -> KernelResult<bool> {
        workspace::set_confirmed(&self.kernel, set_id)
    }

    /// The ids of the sets that are running.
    pub fn set_running(&self) -> KernelResult<Vec<String>> {
        workspace::set_running(&self.kernel)
    }

    /// Resets a set's gates: runs and reviews before now stop counting.
    pub fn reset_set(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        workspace::reset_set(&self.kernel, context, input)
    }

    /// The verify gate of a set under the commands and the review of its first member, after its reset.
    pub fn set_gate(&self, set_id: &str, commands_sha: &str) -> KernelResult<Value> {
        workspace::set_gate(&self.kernel, set_id, commands_sha)
    }

    /// `removeSetMember`: an unconfirmed member leaves its set and the set's gates are reset.
    pub fn remove_set_member(
        &self,
        context: &MutationContext,
        set_id: &str,
        repo: &str,
    ) -> KernelResult<Value> {
        workspace::remove_set_member(
            &self.kernel,
            context,
            &serde_json::json!({"setId": set_id, "repo": repo}),
        )
    }

    /// `setGate` with each repo's own commands sha (`sha_of` gives it; '' accepts any commands).
    pub fn set_gate_by(
        &self,
        set_id: &str,
        sha_of: &dyn Fn(&str) -> String,
    ) -> KernelResult<Value> {
        workspace::set_gate_by(&self.kernel, set_id, sha_of)
    }

    /// The set a subject id names: the set's own id or a member's integration id.
    pub fn set_of_subject(&self, subject_id: &str) -> KernelResult<Option<String>> {
        workspace::set_of_subject(&self.kernel, subject_id)
    }

    /// The `set` member of an integration that belongs to an integration set (`None` for any other).
    pub fn integration_set_view(&self, integration_id: &str) -> KernelResult<Option<Value>> {
        workspace::integration_set_view(&self.kernel, integration_id)
    }

    /// The newest `limit` integration sets with their members in merge order (the status member `integrationSets`).
    pub fn integration_sets_view(&self, limit: i64) -> KernelResult<Value> {
        workspace::integration_sets_view(&self.kernel, limit)
    }

    /// The open cleanup offers of a workspace, newest first (see `workspace::cleanup_offers_view`).
    pub fn cleanup_offers_view(&self) -> KernelResult<Value> {
        workspace::cleanup_offers_view(&self.kernel)
    }

    /// The gate and each live member's verify of the newest `limit` sets (see `workspace::set_verify_view`).
    pub fn set_verify_view(
        &self,
        limit: i64,
        sha_of: &dyn Fn(&str) -> String,
    ) -> KernelResult<Value> {
        workspace::set_verify_view(&self.kernel, limit, sha_of)
    }
}

//! The public surface of the controller core: one Rust method per public `ControllerCore` method of
//! src/controller/core.ts (same name in snake_case), delegating to `areas/<area>.rs`.
//!
//! Structured inputs and outputs travel as `serde_json::Value` in the camelCase shapes of the TypeScript records, so
//! the parity sequences compare them as JSON. The area packages fill the bodies in their area files; nothing here
//! changes when they do.

use crate::areas::{
    actors, agents, findings, integrations, links, message_notices, messages, operator_grants,
    operator_proposals, operator_runs, panes, pauses, plan_packages, plans, prompt_relay, reports,
    reviews, status,
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

    /// `ControllerCore.reviewsToRelease`.
    pub fn reviews_to_release(&self, credential: &str) -> KernelResult<Value> {
        reviews::reviews_to_release(&self.kernel, credential)
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

    /// `ControllerCore.integration`.
    pub fn integration(&self, integration_id: &str) -> KernelResult<Value> {
        integrations::integration(&self.kernel, integration_id)
    }

    /// `ControllerCore.integrationCommitInfo`.
    pub fn integration_commit_info(&self, integration_id: &str) -> KernelResult<Value> {
        integrations::integration_commit_info(&self.kernel, integration_id)
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

    /// `ControllerCore.enqueueMessage`.
    pub fn enqueue_message(&self, context: &MutationContext, input: &Value) -> KernelResult<Value> {
        messages::enqueue_message(&self.kernel, context, input)
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
}

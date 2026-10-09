/* Record, row and option shapes and the limits of the controller core. */
import {
  MESSAGING_TIMER_NAMES,
  type DeferralReason,
  type MessageState,
} from "./messaging.js";
import { type PackageView } from "../plans.js";
import { type ExternalRefKind, type NexoraState } from "../nexora.js";
import type {
  AgentRecord,
  OperatorGrantEndReason,
  OperatorGrantKind,
  OperatorProposalKind,
  OperatorProposalRecord,
  OperatorProposalState,
  OperatorRunStatus,
  PauseRecord,
  PromptRelayState,
  Role,
} from "./types.js";

export const ROLE_NAME_PATTERN = /^[a-z][a-z0-9-]{0,31}$/;

export const ROLE_KINDS: readonly string[] = [
  "PM",
  "Developer",
  "Verifier",
  "Supervisor",
];

export const MAX_MESSAGE_BYTES = 16 * 1024;

/** What a restart proposal stores as its command text; the hash covers it with the kind and the force flag. */
export const RESTART_COMMAND_TEXT = "restart";

export const MAX_OPERATOR_TAIL_BYTES = 12288;

export const MAX_INPUT_CLEAR_BYTES = 64 * 1024;

export const MAX_SUMMARY_MESSAGES = 50;

export const MAX_SUMMARY_BODY = 2000;

export const MAX_SUMMARY_WORK = 200;

export const MAX_SUMMARY_LINKS = 50;

export const MAX_SUMMARY_BYTES = 32 * 1024;

export const MAX_OBJECTIVE_BYTES = 8 * 1024;

export const TRUNCATION_MARKER = "[truncated]";

export const SAFE_ID_PATTERN = /^[A-Za-z0-9._:-]{1,128}$/;

export const VISIBLE_TEXT = /[\p{L}\p{N}\p{P}\p{S}]/u;

export const BLANK_FILLERS = /[\u2800\u115f\u1160\u3164\uffa0]/g;

export const UNSAFE_TEXT =
  /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]/u;

export const TIMER_NAMES = MESSAGING_TIMER_NAMES;

export interface AgentPaneInput {
  readonly agentId: string;
  readonly workspaceId: string | null;
  readonly paneId: string | null;
  readonly worktreePath: string | null;
  readonly branch: string | null;
  readonly baseSha: string | null;
  /** The task the worker was spawned for; omitted (never undefined) when it has none, so stored requests replay. */
  readonly taskRef?: string;
  readonly taskTitle?: string;
  /** Herdr's terminal id of the pane; omitted when it is not known. Replaced, never kept, when the pane changes. */
  readonly terminalId?: string;
}

export interface AgentPaneRecord extends Omit<
  AgentPaneInput,
  "taskRef" | "taskTitle" | "terminalId"
> {
  readonly generation: number;
  readonly taskRef: string | null;
  readonly taskTitle: string | null;
}

export const MAX_REPORT_SUMMARY_BYTES = 1000;

/** After this many rejected reports for one agent generation, further reports are refused without a new row. */
export const MAX_REJECTED_REPORTS = 100;

export type ReportReason =
  | "agent_changed"
  | "no_branch"
  | "no_base"
  | "commit_missing"
  | "not_new_on_branch"
  | "not_on_branch";

/** What the controller checked in git for a report; the agent never supplies it. */
export interface ReportEvidence {
  readonly generation: number;
  readonly branch: string;
  readonly baseSha: string | null;
  readonly commitExists: boolean;
  readonly branchTip: string | null;
  readonly isAncestorOfTip: boolean;
  readonly isAncestorOfBase: boolean;
  readonly checkedAt: string;
}

export interface AgentReportRecord {
  readonly reportId: string;
  readonly sequence: number;
  readonly agentId: string;
  readonly generation: number;
  /** The actor whose token made the claim: the claimed identity. */
  readonly actorId: string;
  readonly commitSha: string;
  readonly branch: string | null;
  readonly summary: string;
  readonly state: "accepted" | "rejected";
  readonly reason: ReportReason | null;
  readonly evidence: ReportEvidence;
  readonly notifiedMessageId: string | null;
  readonly createdAt: string;
}

export interface ReportResult {
  readonly record: AgentReportRecord;
  /** True when this accepted commit was already reported by this agent generation: nothing new was written. */
  readonly duplicate: boolean;
}

export interface AgentReportRow {
  readonly report_id: string;
  readonly sequence: number;
  readonly agent_id: string;
  readonly generation: number;
  readonly actor_id: string;
  readonly commit_sha: string;
  readonly branch: string | null;
  readonly summary: string;
  readonly state: "accepted" | "rejected";
  readonly reason: ReportReason | null;
  readonly evidence_json: string;
  readonly notified_message_id: string | null;
  readonly created_at: string;
}

export const MAX_REVIEW_TEXT_BYTES = 4000;

/** Rounds that ended in a verdict; failed and cancelled rounds do not use the budget. */
export const MAX_REVIEW_ROUNDS = 5;

export type ReviewState =
  "started" | "passed" | "findings" | "failed" | "cancelled";

export interface ReviewRecord {
  readonly reviewId: string;
  readonly sequence: number;
  readonly round: number;
  readonly reportId: string | null;
  readonly integrationId: string | null;
  readonly planId: string | null;
  readonly planRevision: number | null;
  readonly commitSha: string;
  readonly baseSha: string;
  readonly authorAgentId: string | null;
  readonly authorActorId: string | null;
  readonly requestedByActorId: string;
  readonly reviewerRole: string;
  readonly reviewerAgentId: string;
  readonly reviewerActorId: string;
  readonly state: ReviewState;
  readonly verdictText: string | null;
  readonly failureReason: string | null;
  readonly notifiedMessageId: string | null;
  readonly createdAt: string;
  readonly completedAt: string | null;
}

export interface ReviewRow {
  readonly review_id: string;
  readonly sequence: number;
  readonly round: number;
  readonly subject_report_id: string | null;
  readonly subject_integration_id: string | null;
  readonly subject_plan_id: string | null;
  readonly subject_plan_revision: number | null;
  readonly commit_sha: string;
  readonly base_sha: string;
  readonly author_agent_id: string | null;
  readonly author_actor_id: string | null;
  readonly requested_by_actor_id: string;
  readonly reviewer_role: string;
  readonly reviewer_agent_id: string;
  readonly reviewer_actor_id: string;
  readonly state: ReviewState;
  readonly verdict_text: string | null;
  readonly failure_reason: string | null;
  readonly notified_message_id: string | null;
  readonly created_at: string;
  readonly completed_at: string | null;
}

export const PLAN_NOTICE_BUDGET_BYTES = 12 * 1024;

export const MAX_PLAN_BODY_BYTES = 32 * 1024;

export const MAX_PLAN_PACKAGES = 20;

export const PLAN_PACKAGE_ID = /^[a-z][a-z0-9-]{0,31}$/;

export type PlanTier = "normal" | "high_risk";

export type PlanState = "draft" | "in_review" | "approved" | "superseded";

export type PackageProgress =
  | "unassigned"
  | "assigned"
  | "reported"
  | "findings"
  | "reviewed"
  | "integrated";

export interface PlanRecord {
  readonly planId: string;
  readonly sequence: number;
  readonly title: string;
  readonly tier: PlanTier;
  readonly state: PlanState;
  readonly requestedBy: string;
  readonly architectAgentId: string | null;
  readonly currentRevision: number;
  readonly approvedRevision: number | null;
  readonly supersedesPlanId: string | null;
  readonly cancelledAt: string | null;
  readonly createdAt: string;
  readonly updatedAt: string;
}

export interface PlanRevisionRecord {
  readonly revision: number;
  readonly baseSha: string;
  readonly bodyJson: string;
  readonly bodySha: string;
  readonly authorAgentId: string;
  readonly createdAt: string;
}

export interface PlanPackageRecord {
  readonly packageId: string;
  readonly assigneeAgentId: string | null;
  readonly assignedAt: string | null;
  readonly assignmentMessageId: string | null;
  readonly cancelledAt: string | null;
  readonly progress: PackageProgress;
}

export interface ExternalLinkRecord {
  readonly refKind: ExternalRefKind;
  readonly refId: string;
  readonly system: "nexora";
  readonly externalId: string;
  readonly syncedState: NexoraState;
  readonly boundAgentId: string | null;
  /** When the agent was bound; only reports accepted at or after it count for the requirement. */
  readonly boundAt: string | null;
  readonly linkedBy: string;
  readonly linkedAt: string;
  readonly syncedAt: string;
  /** Derived from ledger facts, never stored; null when the ledger has no wanted state for the ref. */
  readonly wanted: NexoraState | null;
  readonly drift: boolean;
}

export interface ExternalLinkRow {
  readonly ref_kind: ExternalRefKind;
  readonly ref_id: string;
  readonly system: "nexora";
  readonly external_id: string;
  readonly synced_state: NexoraState;
  readonly bound_agent_id: string | null;
  readonly bound_at: string | null;
  readonly linked_by: string;
  readonly linked_at: string;
  readonly synced_at: string;
}

export interface PlanCancelResult {
  readonly planId: string;
  readonly packageId: string | null;
  /** The packages this call cancelled. */
  readonly cancelledPackages: readonly string[];
  /** The plan review this call cancelled, and its reviewer, whom the caller releases. */
  readonly reviewId: string | null;
  readonly reviewerAgentId: string | null;
  /** The agents the controller queued the notice for. */
  readonly notified: readonly string[];
}

export type PlanNoticeRef =
  | { readonly planId: string; readonly kind: "approved" | "needs_attention" }
  | {
      readonly planId: string;
      readonly kind: "signed_off";
      readonly integrationId: string;
    };

export interface PlanSignoffRecord {
  readonly integrationId: string;
  readonly architectAgentId: string;
  readonly summary: string;
  readonly createdAt: string;
  /** Reports of the integration that are not packages of the plan. */
  readonly extraReports: readonly string[];
}

export interface PlanStatusEntry {
  readonly planId: string;
  readonly title: string;
  readonly tier: PlanTier;
  readonly state: PlanState;
  readonly cancelled: boolean;
  readonly architectAgentId: string | null;
  readonly createdAt: string;
  readonly updatedAt: string;
  /** Package counts by derived progress; cancelled packages are counted under `cancelled` only. */
  readonly packages: Readonly<Record<string, number>>;
  readonly signoffs: readonly {
    readonly integrationId: string;
    readonly createdAt: string;
  }[];
}

/** A plan with its approved revision (or the current one while it is not approved), its packages and its sign-offs. */
export interface PlanDetail {
  readonly plan: PlanRecord;
  readonly revision: PlanRevisionRecord | null;
  readonly packages: readonly PlanPackageRecord[];
  readonly signoffs: readonly PlanSignoffRecord[];
}

export interface PlanRow {
  readonly plan_id: string;
  readonly sequence: number;
  readonly title: string;
  readonly tier: PlanTier;
  readonly state: PlanState;
  readonly requested_by: string;
  readonly architect_agent_id: string | null;
  readonly current_revision: number;
  readonly approved_revision: number | null;
  readonly supersedes_plan_id: string | null;
  readonly cancelled_at: string | null;
  readonly created_at: string;
  readonly updated_at: string;
}

export const MAX_INTEGRATION_REPORTS = 20;

export const MAX_CONFLICT_FILES = 50;

export const MAX_CONFLICT_PATH_CHARS = 200;

/** Room for the cut marker: `...#` and twelve hex digits. */
export const PATH_CUT_MARK_CHARS = 16;

export interface CoverageCandidate {
  readonly reportId: string;
  readonly commitSha: string;
  /** Heads of the other integrations (any state) that held this report. */
  readonly integrationHeads: readonly string[];
}

export type IntegrationState =
  "running" | "merged" | "conflicted" | "failed" | "confirmed" | "discarded";

export interface IntegrationRecord {
  readonly integrationId: string;
  readonly sequence: number;
  readonly baseSha: string;
  readonly branch: string;
  readonly requestedBy: string;
  readonly state: IntegrationState;
  readonly headSha: string | null;
  readonly conflictReportId: string | null;
  readonly conflictFiles: readonly string[] | null;
  readonly conflictFilesOmitted: number | null;
  readonly failureReason: string | null;
  readonly reports: readonly {
    readonly reportId: string;
    readonly agentId: string;
    readonly commitSha: string;
  }[];
  readonly createdAt: string;
  readonly completedAt: string | null;
}

export interface IntegrationRow {
  readonly integration_id: string;
  readonly sequence: number;
  readonly base_sha: string;
  readonly branch: string;
  readonly requested_by: string;
  readonly state: IntegrationState;
  readonly head_sha: string | null;
  readonly conflict_report_id: string | null;
  readonly conflict_files_json: string | null;
  readonly conflict_files_omitted: number | null;
  readonly failure_reason: string | null;
  readonly created_at: string;
  readonly completed_at: string | null;
}

export type IntegrationOutcome =
  | { readonly kind: "merged"; readonly headSha: string }
  | {
      readonly kind: "conflicted";
      readonly reportId: string;
      readonly files: readonly string[];
      readonly omitted: number;
    }
  | { readonly kind: "failed"; readonly reason: string };

export const MAX_FINDING_EVIDENCE_BYTES = 1500;

export const MAX_FINDING_CORRECTION_BYTES = 600;

export const MAX_FINDING_CONDITION_BYTES = 300;

export const FINDING_INTERVENTIONS = 2;

export const FINDING_SEVERITIES = [
  "info",
  "low",
  "medium",
  "high",
  "critical",
] as const;

export type AgentFindingSeverity = (typeof FINDING_SEVERITIES)[number];

export type AgentFindingState = "open" | "resolved" | "escalated" | "cancelled";

export type AgentFindingReason =
  "second_unresolved" | "timed_out" | "target_ended" | "raiser_ended";

export type AgentFindingNoticeEvent =
  "raised" | "resolved" | "escalated" | "cancelled";

export interface AgentFindingRecord {
  readonly findingId: string;
  readonly sequence: number;
  readonly targetAgentId: string;
  readonly raisedByAgentId: string;
  readonly severity: AgentFindingSeverity;
  readonly evidence: string;
  readonly requestedCorrection: string;
  readonly resolutionCondition: string;
  readonly state: AgentFindingState;
  readonly interventions: number;
  readonly stateReason: AgentFindingReason | null;
  readonly createdAt: string;
  readonly closedAt: string | null;
  readonly deliveries: readonly {
    readonly attempt: number;
    readonly messageId: string;
    readonly messageState: MessageState | null;
  }[];
  readonly checks: readonly {
    readonly afterIntervention: number;
    readonly result: "resolved" | "unresolved" | "timed_out";
    readonly evidence: string;
    readonly createdAt: string;
  }[];
}

export interface AgentFindingNoticeRecord {
  readonly noticeId: string;
  readonly sequence: number;
  readonly findingId: string;
  readonly event: AgentFindingNoticeEvent;
  readonly messageId: string | null;
}

export interface AgentFindingRow {
  readonly finding_id: string;
  readonly sequence: number;
  readonly target_agent_id: string;
  readonly raised_by_agent_id: string;
  readonly raised_by_actor_id: string;
  readonly severity: AgentFindingSeverity;
  readonly evidence_text: string;
  readonly requested_correction: string;
  readonly resolution_condition: string;
  readonly state: AgentFindingState;
  readonly interventions: number;
  readonly state_reason: AgentFindingReason | null;
  readonly created_at: string;
  readonly closed_at: string | null;
}

export interface AgentFindingNoticeRow {
  readonly notice_id: string;
  readonly sequence: number;
  readonly finding_id: string;
  readonly event: AgentFindingNoticeEvent;
  readonly message_id: string | null;
}

export const ESCALATION_REASON_TEXT: Readonly<Record<string, string>> = {
  second_unresolved: "the target did not recover after two corrections",
  timed_out:
    "no check was recorded before the deadline (the supervisor did not check, or the correction was not acknowledged)",
};

export const CANCEL_REASON_TEXT: Readonly<Record<string, string>> = {
  target_ended: "the target agent ended",
  raiser_ended: "the supervisor that raised it ended",
};

export const FAILURE_REASON_POINTS = 200;

/** Message ids a loss notice names; the rest is counted. */
export const LOST_NOTICE_IDS = 10;

/** Message ids a loss event stores. */
export const LOST_EVENT_IDS = 50;

export const SEED_MESSAGES = 20;

export const SEED_REPORTS = 10;

export const SEED_FINDINGS = 5;

export interface AgentSeedData {
  readonly agentId: string;
  readonly roleName: string;
  readonly kind: AgentRecord["kind"];
  readonly state: "active" | "ended";
  readonly generation: number;
  readonly branch: string | null;
  readonly baseSha: string | null;
  /** The task the agent was spawned for; null for an agent without one or whose pane row is gone. */
  readonly taskRef?: string | null;
  readonly taskTitle?: string | null;
  readonly messages: readonly {
    readonly messageId: string;
    readonly sender: string;
    readonly state: MessageState;
    readonly stateReason: string | null;
    readonly body: string;
  }[];
  readonly messagesOmitted: number;
  readonly reports: readonly {
    readonly reportId: string;
    readonly commitSha: string;
    readonly branch: string | null;
    readonly summary: string;
  }[];
  readonly reportsOmitted: number;
  readonly lastAcceptedCommit: string | null;
  readonly findingsOmitted: number;
  readonly findings: readonly {
    readonly findingId: string;
    readonly severity: string;
    readonly requestedCorrection: string;
    readonly interventions: number;
  }[];
  /** Work packages the agent holds; a replacement takes them over. */
  readonly packages: readonly {
    readonly planId: string;
    readonly packageId: string;
    readonly architectAgentId: string | null;
    readonly view: PackageView | null;
  }[];
}

export interface PmRestartSummary {
  readonly objective: unknown;
  readonly openWork: readonly {
    readonly workItemId: string;
    readonly title: string;
    readonly role: string;
    readonly state: string;
    readonly owner: string | null;
    readonly blockers: readonly string[];
  }[];
  readonly messages: readonly {
    readonly messageId: string;
    readonly from: string;
    readonly body: string;
    readonly state: string;
  }[];
  /** Plans that are not finished, cancelled or superseded. Absent in summaries recorded before plans existed. */
  readonly plans?: readonly {
    readonly planId: string;
    readonly title: string;
    readonly tier: string;
    readonly state: string;
    readonly packages: number;
    readonly signedOff: readonly string[];
  }[];
  /** Integrations still in the merged state: the PM runs `integrate confirm` when the user has merged them. */
  readonly integrations?: readonly {
    readonly integrationId: string;
    readonly branch: string;
    readonly headSha: string | null;
  }[];
  /** Nexora links with their synced and wanted states, drifted first. Absent in summaries recorded before links existed. */
  readonly links?: readonly {
    readonly refKind: ExternalRefKind;
    readonly refId: string;
    readonly externalId: string;
    readonly syncedState: NexoraState;
    readonly wanted: NexoraState | null;
    readonly drift: boolean;
    readonly boundAgentId: string | null;
  }[];
  readonly truncated: boolean;
  readonly summarizedGeneration: number;
  readonly generatedAt: string;
}

export const SUPERVISION_CHECK_TEXT =
  "Routine check from the controller. Run cstan status, then cstan observe each active worker, and apply your standing instructions. Raise a finding only for the same failing command repeated, a step that cannot work, or no progress while looking busy; otherwise raise nothing. Then cstan ack this message.";

export interface MessageRejection {
  readonly rejected: true;
  readonly code: string;
  readonly message: string;
}

export interface AgentRow {
  readonly agent_id: string;
  readonly role_name: string;
  readonly kind: AgentRecord["kind"];
  readonly seat_id: string;
  readonly actor_id: string;
  readonly generation: number;
  readonly state: "active" | "ended";
  readonly last_activity_at: string;
}

/** The longest free-text answer the controller accepts: short enough that it cannot wrap the worker's text field. */
export const PROMPT_RELAY_TEXT_MAX_CHARS = 200;

export interface PromptRelayRow {
  readonly relay_id: string;
  readonly sequence: number;
  readonly agent_id: string;
  readonly pane_id: string;
  readonly host_kind: string;
  readonly prompt_text: string;
  readonly options_json: string;
  readonly prompt_sha: string;
  readonly captured_by_actor_id: string;
  readonly captured_at: string;
  readonly expires_at: string;
  readonly state: PromptRelayState;
  readonly answer_kind: "option" | "esc" | "text" | null;
  readonly answer_option: number | null;
  readonly answer_widens_permissions: number | null;
  readonly answer_text: string | null;
  readonly answered_by_actor_id: string | null;
  readonly answered_at: string | null;
  readonly outcome_reason: string | null;
  readonly keys_json: string | null;
}

export interface OperatorProposalRow {
  readonly proposal_id: string;
  readonly sequence: number;
  readonly kind: OperatorProposalKind;
  readonly command: string;
  readonly command_sha: string;
  readonly reason: string;
  readonly force_restart: number;
  readonly proposer_agent_id: string;
  readonly proposer_actor_id: string;
  readonly state: OperatorProposalState;
  readonly auto_rule: string | null;
  readonly decided_by_actor_id: string | null;
  readonly decided_at: string | null;
  readonly decision_note: string | null;
  readonly created_at: string;
  readonly updated_at: string;
}

export interface OperatorRunRow {
  readonly proposal_id: string;
  readonly started_at: string;
  readonly finished_at: string | null;
  readonly status: OperatorRunStatus;
  readonly exit_code: number | null;
  readonly duration_ms: number | null;
  readonly output_tail: string;
  readonly output_truncated: number;
  readonly notified_message_id: string | null;
  readonly pgid: number | null;
  readonly leader_start: string | null;
  readonly orphan_cleared_at: string | null;
  readonly full_auto: number;
}

export interface OperatorGrantRow {
  readonly grant_id: string;
  readonly sequence: number;
  readonly kind: OperatorGrantKind;
  readonly text: string;
  readonly command_sha: string;
  readonly created_by: string;
  readonly source_proposal_id: string;
  readonly created_at: string;
  readonly expires_at: string;
  readonly revoked_at: string | null;
  readonly ended_reason: OperatorGrantEndReason | null;
}

export type OperatorClaimRefusal =
  "not_approved" | "expired" | "busy" | "run_paused";

export type OperatorClaim =
  | { readonly claimed: true; readonly proposal: OperatorProposalRecord }
  | {
      readonly claimed: false;
      readonly reason: OperatorClaimRefusal;
      readonly detail?: string;
    };

export interface MessageRow {
  readonly message_id: string;
  readonly sequence: number;
  readonly recipient_agent_id: string;
  readonly recipient_generation: number;
  readonly sender_actor_id: string;
  readonly body: string;
  readonly state: MessageState;
  readonly state_version: number;
  readonly queued_at: string;
  readonly deferred_at: string | null;
  readonly deferred_reason: DeferralReason | null;
  readonly deferral_count: number;
  readonly sent_at: string | null;
  readonly acked_at: string | null;
  readonly send_attempts: number;
  readonly state_reason: string | null;
  readonly notified_at: string | null;
  readonly last_notified_at: string | null;
  readonly action_needed: number;
}

export interface MutationEvent {
  readonly entityType: string;
  readonly entityId: string;
  readonly stateVersion: number;
  readonly fromState?: string;
  readonly toState?: string;
  readonly details?: unknown;
}

export interface MutationOutput<T> {
  readonly value: T;
  readonly event: MutationEvent;
  /** Further events of the same mutation, written after `event` with the same state version. */
  readonly extraEvents?: readonly MutationEvent[];
}

export interface ActorInput {
  readonly displayName: string;
  readonly role: Role;
  readonly seatId?: string;
}

export interface ReadinessResult {
  readonly ready: boolean;
  readonly reasons: readonly string[];
  readonly workItemId: string;
  readonly inputRevision: number;
}

export interface SupervisionActivity {
  readonly supervisor: {
    readonly agentId: string;
    readonly state: string;
  } | null;
  readonly lastCheck: {
    readonly messageId: string;
    readonly state: string;
    readonly queuedAt: string;
    readonly ackedAt: string | null;
  } | null;
  readonly openFindings: number;
}

export interface ControllerStatus {
  readonly projectId: string;
  readonly run: { readonly state: string; readonly stateVersion: number };
  readonly pause: {
    readonly run: PauseRecord | null;
    readonly agents: readonly PauseRecord[];
  };
  readonly stateVersion: number;
  readonly inputRevision: number;
  readonly supervision: {
    readonly enabled: boolean;
    readonly health: "healthy" | "evaluating" | "degraded";
    readonly targetEpoch: number;
    readonly checkpointEpoch: number | null;
    readonly checkpointAssignmentId: string | null;
    readonly replacementAttempts: number;
  };
  readonly roles: readonly {
    readonly role: string;
    readonly seatId: string;
    readonly seatState: string;
    readonly actorActive: boolean;
    readonly sessionState: string | null;
    readonly assignmentId: string | null;
  }[];
  readonly work: readonly {
    readonly workItemId: string;
    readonly title: string;
    readonly role: string;
    readonly state: string;
    readonly owner: string | null;
    readonly blockers: readonly string[];
    readonly nextLegalActions: readonly string[];
  }[];
  readonly plans: readonly PlanStatusEntry[];
  readonly findings: readonly {
    readonly findingId: string;
    readonly affectedWorkItemId: string;
    readonly affectedSeatId: string;
    readonly affectedAssignmentId: string;
    readonly affectedGeneration: number;
    readonly fingerprint: string;
    readonly severity: string;
    readonly state: string;
    readonly evidence: unknown;
    readonly requestedCorrection: string;
    readonly acknowledgementDeadline: string;
    readonly resolutionCondition: string;
    readonly escalationRoute: string;
    readonly interventionCount: number;
    readonly reopenedFromFindingId: string | null;
  }[];
  readonly evidence: readonly {
    readonly candidateId: string;
    readonly commitSha: string;
    readonly reportHash: string;
    readonly developerEvidence: readonly string[] | null;
    readonly evidenceRef: string | null;
    readonly verifierEvidence: readonly {
      readonly evidenceId: string;
      readonly criterion: string;
      readonly passed: boolean;
      readonly artifactRef: string;
      readonly observation: string | null;
      readonly exitStatus: number | null;
      readonly evidenceHash: string;
    }[];
  }[];
  readonly finalVerification: readonly {
    readonly workItemId: string;
    readonly assignmentId: string;
    readonly commitSha: string;
    readonly evidence: readonly {
      readonly evidenceId: string;
      readonly criterion: string;
      readonly passed: boolean;
      readonly artifactRef: string;
      readonly observation: string | null;
      readonly exitStatus: number | null;
      readonly evidenceHash: string;
    }[];
  }[];
}

import type {
  DeferralReason,
  MessageState,
  AttentionEpisode,
  MessagingAction,
} from "./messaging.js";
export const roles = [
  "operator",
  "controller",
  "PM",
  "Developer",
  "Verifier",
  "Supervisor",
] as const;

export type Role = (typeof roles)[number];
export type InputKind =
  "project_config" | "task_brief" | "acceptance_criteria" | "policy" | "plan";
export type WorkState =
  | "pending"
  | "ready"
  | "running"
  | "awaiting_verification"
  | "accepted"
  | "blocked"
  | "canceled"
  | "failed";
export type AssignmentState =
  | "created"
  | "dispatched"
  | "acknowledged"
  | "running"
  | "reported"
  | "completed"
  | "revoked"
  | "failed";
export type FindingState =
  | "detected"
  | "reported"
  | "acknowledged"
  | "correcting"
  | "resolved"
  | "disputed"
  | "escalated";
export type RunState =
  "active" | "paused" | "canceling" | "canceled" | "completed" | "failed";
export type EntityType =
  "work_item" | "assignment_attempt" | "finding" | "run_control" | "command";
export type Capability =
  | "project:inputs:write"
  | "work:write"
  | "work:assign"
  | "work:report"
  | "candidate:verify"
  | "candidate:accept"
  | "finding:write"
  | "run:control"
  | "usage:write"
  | "actor:manage"
  | "message:send"
  | "message:receive"
  | "message:resolve"
  | "report:submit"
  | "review:request"
  | "review:submit"
  | "plan:write"
  | "plan:read"
  | "operator:propose"
  | "operator:decide"
  | "operator:read"
  | "finding:raise"
  | "finding:check"
  | "agent:observe"
  | "recovery:write"
  | "controller:reconcile";

export interface MutationContext {
  readonly credential: string;
  readonly requestId: string;
  readonly idempotencyKey: string;
  readonly expectedVersion: number;
  readonly inputRevision: number;
}

export interface ProjectInput {
  readonly kind: InputKind;
  readonly content: unknown;
}

export interface InitialProject {
  readonly projectId: string;
  readonly name: string;
  readonly ownerCredential: string;
  readonly initialInputs: readonly ProjectInput[];
}

export interface WorkItemInput {
  readonly workItemId: string;
  readonly title: string;
  readonly description: string;
  readonly requiredRole: Exclude<Role, "operator" | "controller">;
  readonly parentWorkItemId?: string;
  readonly acceptanceCriteria?: readonly string[];
  readonly finalVerification?: boolean;
  readonly findingId?: string;
}

export interface SeatInput {
  readonly seatId: string;
  readonly name: string;
  readonly role: Exclude<Role, "operator" | "controller">;
}

export interface RoleDefinitionInput {
  readonly name: string;
  readonly kind: Exclude<Role, "operator" | "controller">;
  readonly host: string;
  readonly configHash: string;
}

export interface RoleDefinition extends RoleDefinitionInput {
  readonly state: "active" | "retired";
}

export interface RoleSyncResult {
  readonly changed: boolean;
  readonly inserted: readonly string[];
  readonly updated: readonly string[];
  readonly reactivated: readonly string[];
  readonly retired: readonly string[];
}

export interface AssignmentResult {
  readonly assignmentId: string;
  readonly attempt: number;
  readonly generation: number;
  readonly commandId: string;
  readonly workItemId: string;
  readonly seatId: string;
  readonly inputRevision: number;
}

export interface ReplacementInput {
  readonly workItemId: string;
  readonly assignmentId: string;
  readonly recoveryId: string;
  readonly reason: string;
}

export interface CandidateInput {
  readonly candidateId: string;
  readonly assignmentId: string;
  readonly commitSha: string;
  readonly baseSha: string;
  readonly evidence: readonly string[];
  readonly changedScope: readonly string[];
  readonly limitations: readonly string[];
}

export interface EvidenceInput {
  readonly evidenceId: string;
  readonly candidateId: string;
  readonly criterion: string;
  readonly passed: boolean;
  readonly artifactRef: string;
  readonly observation: string;
  readonly exitStatus: number;
}

export interface BridgeIdentity {
  readonly commandId: string;
  readonly assignmentId: string;
  readonly attempt: number;
  readonly generation: number;
}

export interface BridgeReceipt extends BridgeIdentity {
  readonly sequence: number;
  readonly type:
    | "accepted"
    | "submitted"
    | "working"
    | "tool_started"
    | "tool_completed"
    | "aborted"
    | "dispatch_error"
    | "completed"
    | "agent_end_without_reply";
  readonly role: string;
  readonly timestamp: string;
  readonly [field: string]: unknown;
}

export interface ControllerOptions {
  readonly stateDirectory: string;
  readonly project: InitialProject;
  readonly workspaceRoot?: string;
  readonly runtimeWorkspacePath?: string;
  readonly clock?: () => Date;
}

export type AgentKind = Exclude<Role, "operator" | "controller">;

export interface AgentInput {
  readonly agentId: string;
  readonly roleName: string;
  readonly seatId: string;
  readonly actorId: string;
}

export interface AgentRecord {
  readonly agentId: string;
  readonly roleName: string;
  readonly kind: AgentKind;
  readonly seatId: string;
  readonly actorId: string;
  readonly generation: number;
  readonly state: "active" | "ended";
  readonly lastActivityAt: string;
}

export interface ReplacedAgent {
  readonly agentId: string;
  readonly generation: number;
  readonly actorId: string;
  readonly credential: string;
  readonly cancelledMessageIds: readonly string[];
}

export interface MessageInput {
  readonly recipientAgentId: string;
  readonly body: string;
}

export interface MessageRecord {
  readonly messageId: string;
  readonly sequence: number;
  readonly recipientAgentId: string;
  readonly recipientGeneration: number;
  readonly senderActorId: string;
  readonly body: string;
  readonly state: MessageState;
  readonly stateVersion: number;
  readonly queuedAt: string;
  readonly deferredAt: string | null;
  readonly deferredReason: DeferralReason | null;
  readonly sentAt: string | null;
  readonly ackedAt: string | null;
  readonly sendAttempts: number;
  readonly stateReason: string | null;
  readonly notifiedAt: string | null;
  readonly lastNotifiedAt: string | null;
}

export interface MessageRejectionRecord {
  readonly rejectionId: string;
  readonly messageId: string | null;
  readonly action: string;
  readonly code: string;
  readonly fromState: string | null;
  readonly attemptedState: string | null;
  readonly actorId: string;
  readonly reason: string;
}

export interface MessagingAdvance {
  readonly applied: readonly string[];
  readonly actions: readonly MessagingAction[];
  readonly stalledAgentIds: readonly string[];
  readonly attention: readonly AttentionEpisode[];
}

export interface Identity {
  readonly actorId: string;
  readonly role: Role;
  readonly capabilities: readonly string[];
  readonly agent: AgentRecord | null;
}

export type OperatorProposalKind = "command" | "restart";
export const OPERATOR_PROPOSAL_STATES = [
  "proposed",
  "approved",
  "denied",
  "cancelled",
  "expired",
  "running",
  "finished",
  "failed",
  "timeout",
  "abandoned",
] as const;
export type OperatorProposalState = (typeof OPERATOR_PROPOSAL_STATES)[number];
export type OperatorRunStatus =
  "running" | "ok" | "failed" | "timeout" | "error" | "abandoned";

export interface OperatorRunRecord {
  readonly proposalId: string;
  readonly startedAt: string;
  readonly finishedAt: string | null;
  readonly status: OperatorRunStatus;
  readonly exitCode: number | null;
  readonly durationMs: number | null;
  readonly outputTail: string;
  readonly outputTruncated: boolean;
  readonly notifiedMessageId: string | null;
  readonly pgid: number | null;
  readonly leaderStart: string | null;
  readonly orphanClearedAt: string | null;
}

export interface OperatorProposalRecord {
  readonly proposalId: string;
  readonly sequence: number;
  readonly kind: OperatorProposalKind;
  readonly command: string;
  readonly commandSha: string;
  readonly reason: string;
  readonly forceRestart: boolean;
  readonly proposerAgentId: string;
  readonly proposerActorId: string;
  readonly state: OperatorProposalState;
  readonly autoRule: string | null;
  readonly decidedByActorId: string | null;
  readonly decidedAt: string | null;
  readonly decisionNote: string | null;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly run: OperatorRunRecord | null;
}

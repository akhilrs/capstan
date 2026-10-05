import type { RelayOption } from "../herdr/prompt-relay.js";
import type {
  DeferralReason,
  MessageState,
  AttentionEpisode,
  MessagingAction,
} from "./messaging.js";
/** What covers an agent's input box; mirrors the adapter's classification, and `unknown` when it is not known. */
export type InputBlocker = "permission_prompt" | "dialog" | "unknown";
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
export type RunState =
  "active" | "paused" | "canceling" | "canceled" | "completed" | "failed";
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
  | "prompt:relay"
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

export interface ControllerOptions {
  readonly stateDirectory: string;
  readonly project: InitialProject;
  readonly workspaceRoot?: string;
  readonly runtimeWorkspacePath?: string;
  readonly clock?: () => Date;
  /** Pre-migration backups to keep after a migration; default 3. */
  readonly keepMigrationBackups?: number;
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
  /** True when the run was approved under full auto. */
  readonly fullAuto: boolean;
}

export type OperatorGrantKind = "exact" | "prefix";
export type OperatorGrantEndReason =
  "released" | "restart" | "expired" | "revoked";

/** A PM-approved permission to run the same command, or a command that starts with the same whole words, again without a new approval until it ends. */
export interface OperatorGrantRecord {
  readonly grantId: string;
  readonly sequence: number;
  readonly kind: OperatorGrantKind;
  readonly text: string;
  readonly commandSha: string;
  readonly createdBy: string;
  readonly sourceProposalId: string;
  readonly createdAt: string;
  readonly expiresAt: string;
  readonly revokedAt: string | null;
  readonly endedReason: OperatorGrantEndReason | null;
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
  /** The session grant this proposal's approval created, if any. */
  readonly sessionGrant: OperatorGrantRecord | null;
}

export type PromptRelayState =
  "captured" | "typing" | "answered" | "refused" | "failed" | "expired";

export interface PromptRelayRecord {
  readonly relayId: string;
  readonly sequence: number;
  readonly agentId: string;
  readonly paneId: string;
  readonly hostKind: string;
  readonly promptText: string;
  readonly options: readonly RelayOption[];
  readonly promptSha: string;
  /** The first 12 hex characters of `promptSha`: what the PM shows and passes back. */
  readonly hash12: string;
  readonly capturedByActorId: string;
  readonly capturedAt: string;
  readonly expiresAt: string;
  readonly state: PromptRelayState;
  readonly answer: {
    readonly kind: "option" | "esc" | "text";
    readonly option: number | null;
    readonly widensPermissions: boolean;
    readonly text: string | null;
  } | null;
  readonly answeredByActorId: string | null;
  readonly answeredAt: string | null;
  readonly outcomeReason: string | null;
  readonly keys: readonly string[] | null;
}

/** One open pause: of the whole run (scope "run", no agent) or of one agent. */
export interface PauseRecord {
  readonly scope: "run" | "agent";
  readonly agentId: string | null;
  readonly reason: string;
  readonly actorId: string;
  readonly pausedAt: string;
}

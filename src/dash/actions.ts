/** The dashboard's actions as data: what is offered for a row, the confirm text, and the daemon call. */
import {
  resolutionTarget,
  type MessageState,
  type ResolutionDecision,
} from "../controller/messaging.js";
import { clean } from "./format.js";
import type { AgentRow, MessageRow } from "./model.js";

export type DashAction =
  | { readonly kind: "observe"; readonly agentId: string }
  | {
      readonly kind: "resolve";
      readonly decision: ResolutionDecision;
      readonly message: MessageRow;
    };

export const ACTION_KEYS: Readonly<Record<ResolutionDecision, string>> = {
  retry: "y",
  skip: "s",
  cancel: "c",
};

const DECISIONS: readonly ResolutionDecision[] = ["retry", "skip", "cancel"];

/** The decisions worth offering for a message. The daemon still decides; this only hides keys that cannot work. */
export function availableDecisions(
  message: MessageRow,
): readonly ResolutionDecision[] {
  return DECISIONS.filter((decision) => {
    if (resolutionTarget(decision, message.state as MessageState) === undefined)
      return false;
    return decision !== "retry" || message.recipientActive;
  });
}

export function observeAction(agent: AgentRow): DashAction | undefined {
  return agent.state === "active"
    ? { kind: "observe", agentId: agent.agentId }
    : undefined;
}

export function duplicateWarning(message: MessageRow): boolean {
  return message.state === "sent" || message.state === "unacked";
}

export function confirmText(action: DashAction): string {
  if (action.kind === "observe") return `Observe ${clean(action.agentId)}?`;
  const { decision, message } = action;
  const verb = { retry: "Retry", skip: "Skip", cancel: "Cancel" }[decision];
  const warning =
    decision === "retry" && duplicateWarning(message)
      ? " The recipient may already have received it."
      : "";
  return `${verb} message ${message.messageId} to ${message.recipientAgentId} (state ${message.state})?${warning} Press y again to confirm, any other key cancels.`;
}

/** The daemon command for an action: the same routes `cstan resolve` and `cstan cancel` use. */
export function toWireCall(action: DashAction): {
  readonly command: string;
  readonly args: readonly string[];
} {
  if (action.kind === "observe")
    return { command: "peek", args: [action.agentId, "40"] };
  const { decision, message } = action;
  return decision === "cancel"
    ? { command: "cancel", args: [message.messageId] }
    : { command: "resolve", args: [message.messageId, decision] };
}

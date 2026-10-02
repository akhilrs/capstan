export const MESSAGE_STATES = [
  "queued",
  "deferred",
  "sent",
  "acked",
  "acked_late",
  "unacked",
  "expired",
  "cancelled",
  "failed",
] as const;
export type MessageState = (typeof MESSAGE_STATES)[number];

export const UNRESOLVED_STATES: readonly MessageState[] = [
  "deferred",
  "sent",
  "unacked",
  "expired",
  "failed",
];
export const FINAL_STATES: readonly MessageState[] = [
  "acked",
  "acked_late",
  "cancelled",
];

export const DEFERRAL_REASONS = [
  "agent_busy",
  "agent_blocked",
  "input_not_empty",
] as const;
export type DeferralReason = (typeof DEFERRAL_REASONS)[number];

export const HERDR_STATES = [
  "idle",
  "working",
  "blocked",
  "done",
  "unknown",
] as const;
export type HerdrState = (typeof HERDR_STATES)[number];

export const RESOLUTION_DECISIONS = ["retry", "skip", "cancel"] as const;
export type ResolutionDecision = (typeof RESOLUTION_DECISIONS)[number];

const TRANSITIONS: Readonly<Record<MessageState, readonly MessageState[]>> = {
  queued: ["deferred", "sent", "failed", "cancelled"],
  deferred: ["sent", "expired", "failed", "cancelled"],
  sent: ["acked", "unacked", "cancelled"],
  unacked: ["acked_late", "cancelled"],
  expired: ["cancelled"],
  failed: ["cancelled"],
  acked: [],
  acked_late: [],
  cancelled: [],
};

export function isLegalTransition(
  from: MessageState,
  to: MessageState,
): boolean {
  return TRANSITIONS[from].includes(to);
}

export function isFinalState(state: MessageState): boolean {
  return FINAL_STATES.includes(state);
}

export function isUnresolvedState(state: MessageState): boolean {
  return UNRESOLVED_STATES.includes(state);
}

export function resolutionTarget(
  decision: ResolutionDecision,
  from: MessageState,
): MessageState | undefined {
  if (decision === "retry")
    return isUnresolvedState(from) ? "queued" : undefined;
  return isUnresolvedState(from) || from === "queued" ? "cancelled" : undefined;
}

export function queueHead<
  T extends { readonly state: MessageState; readonly sequence: number },
>(messages: readonly T[]): T | undefined {
  let head: T | undefined;
  for (const message of messages)
    if (
      !isFinalState(message.state) &&
      (head === undefined || message.sequence < head.sequence)
    )
      head = message;
  return head;
}

export interface StateObservation {
  readonly state: HerdrState;
  readonly atMs: number;
}

export interface WaitInterval {
  readonly startMs: number;
  readonly endMs: number | null;
}

export type CountingRule = "ack" | "stall";

function stateAt(
  observations: readonly StateObservation[],
  atMs: number,
): HerdrState {
  let state: HerdrState = "unknown";
  for (const observation of observations)
    if (observation.atMs <= atMs) state = observation.state;
    else break;
  return state;
}

function inWait(waits: readonly WaitInterval[], atMs: number): boolean {
  return waits.some(
    (wait) =>
      wait.startMs <= atMs && (wait.endMs === null || wait.endMs > atMs),
  );
}

/**
 * Milliseconds in [fromMs, toMs) that count for a timer. The "ack" rule
 * (PM acknowledgement and notification) counts time when the agent is not
 * working and all time inside a registered wait. The "stall" rule counts only
 * time when the agent is working outside every registered wait. Observations
 * must be sorted by time; an agent with no observation is "unknown".
 */
export function countedMillis(
  rule: CountingRule,
  fromMs: number,
  toMs: number,
  observations: readonly StateObservation[],
  waits: readonly WaitInterval[],
): number {
  if (toMs <= fromMs) return 0;
  const cuts = new Set<number>([fromMs, toMs]);
  for (const observation of observations)
    if (observation.atMs > fromMs && observation.atMs < toMs)
      cuts.add(observation.atMs);
  for (const wait of waits) {
    if (wait.startMs > fromMs && wait.startMs < toMs) cuts.add(wait.startMs);
    if (wait.endMs !== null && wait.endMs > fromMs && wait.endMs < toMs)
      cuts.add(wait.endMs);
  }
  const points = [...cuts].sort((left, right) => left - right);
  let total = 0;
  for (let index = 0; index + 1 < points.length; index += 1) {
    const start = points[index]!;
    const working = stateAt(observations, start) === "working";
    const waiting = inWait(waits, start);
    const counts = rule === "ack" ? !working || waiting : working && !waiting;
    if (counts) total += points[index + 1]! - start;
  }
  return total;
}

export interface MessagingTimers {
  readonly maxDeferralSeconds: number;
  /** How long a message may wait because its worker is busy or blocked before it expires (and the PM is told). */
  readonly maxBusyDeferralSeconds: number;
  readonly pmAckTimeoutSeconds: number;
  readonly pmNotifyAfterSeconds: number;
  readonly notifyIntervalSeconds: number;
  readonly stallAfterSeconds: number;
  readonly workerAckTimeoutSeconds: number;
  /** An unread message waits this long before an idle PM is woken; 0 turns the wake off. */
  readonly pmWakeAfterSeconds: number;
  readonly pmWakeIntervalSeconds: number;
}

/** The most wake lines the controller types for one unread message. */
export const MAX_PM_WAKES = 5;

export const MESSAGING_TIMER_NAMES: readonly (keyof MessagingTimers)[] = [
  "maxDeferralSeconds",
  "maxBusyDeferralSeconds",
  "pmAckTimeoutSeconds",
  "pmNotifyAfterSeconds",
  "notifyIntervalSeconds",
  "stallAfterSeconds",
  "workerAckTimeoutSeconds",
  "pmWakeAfterSeconds",
  "pmWakeIntervalSeconds",
];

/**
 * Only the timers the message state machine takes. The configuration holds
 * more (such as the finding deadline), and the controller refuses a timer it
 * does not know, so a configured object must be narrowed before it is passed.
 */
export function messagingTimersOf(timers: MessagingTimers): MessagingTimers {
  return Object.fromEntries(
    MESSAGING_TIMER_NAMES.map((name) => [name, timers[name]]),
  ) as unknown as MessagingTimers;
}

export interface AgentFacts {
  readonly agentId: string;
  readonly kind: "PM" | "Developer" | "Verifier" | "Supervisor";
  readonly lastActivityMs: number;
  readonly observations: readonly StateObservation[];
  readonly waits: readonly WaitInterval[];
}

export interface MessageFacts {
  readonly messageId: string;
  readonly recipientAgentId: string;
  readonly state: MessageState;
  readonly sequence: number;
  readonly queuedMs: number;
  readonly sentMs: number | null;
  readonly deferredMs: number | null;
  readonly deferredReason: DeferralReason | null;
  readonly inputClearRecorded: boolean;
  readonly lastNotifiedMs: number | null;
  readonly wakeCount: number;
  readonly lastWakeMs: number | null;
}

export interface DueTransition {
  readonly messageId: string;
  readonly to: "unacked" | "expired";
}

export type MessagingAction =
  | { readonly kind: "wake_pm"; readonly messageId: string }
  | {
      readonly kind: "clear_then_send";
      readonly messageId: string;
      readonly notifyOperator: boolean;
    }
  | {
      readonly kind: "notify_operator";
      readonly messageId: string;
      readonly repeat: boolean;
    };

/** A worker that needs the PM's attention: `stalled` is working without activity, `blocked` waits at a dialog. The episode identifies this one stall. */
export interface AttentionEpisode {
  readonly agentId: string;
  readonly kind: "stalled" | "blocked";
  readonly episodeMs: number;
}

export interface MessagingEvaluation {
  readonly transitions: readonly DueTransition[];
  readonly actions: readonly MessagingAction[];
  readonly stalledAgentIds: readonly string[];
  readonly attention: readonly AttentionEpisode[];
}

/** When the agent's current Herdr state began: the earliest observation of an unbroken run of that state up to `atMs`. */
function stateSince(
  observations: readonly StateObservation[],
  atMs: number,
): number | null {
  const current = stateAt(observations, atMs);
  let since: number | null = null;
  for (const observation of observations) {
    if (observation.atMs > atMs) break;
    if (observation.state === current) since ??= observation.atMs;
    else since = null;
  }
  return since;
}

/**
 * Pure evaluation of every timer rule. It never yields a send: the only
 * delivery-like action is clear_then_send, and the caller must record it
 * before it acts. Retrying a message is an explicit resolution.
 */
export function evaluateMessaging(
  agents: readonly AgentFacts[],
  messages: readonly MessageFacts[],
  nowMs: number,
  timers: MessagingTimers,
): MessagingEvaluation {
  const transitions: DueTransition[] = [];
  const actions: MessagingAction[] = [];
  const stalledAgentIds: string[] = [];
  const attention: AttentionEpisode[] = [];
  for (const agent of agents) {
    const own = messages.filter(
      (message) => message.recipientAgentId === agent.agentId,
    );
    const head = queueHead(own);
    for (const message of own) {
      if (message.state === "sent" && message.sentMs !== null) {
        const due =
          agent.kind === "PM"
            ? countedMillis(
                "ack",
                message.sentMs,
                nowMs,
                agent.observations,
                agent.waits,
              ) >=
              timers.pmAckTimeoutSeconds * 1000
            : nowMs - message.sentMs >= timers.workerAckTimeoutSeconds * 1000;
        if (due)
          transitions.push({ messageId: message.messageId, to: "unacked" });
      }
    }
    if (head?.state === "deferred" && head.deferredMs !== null) {
      // Time spent waiting for a busy or blocked worker is not a delivery failure for minutes: it gets its own, much longer limit. A line with text on it is cleared after the shorter one.
      if (head.deferredReason === "input_not_empty") {
        if (nowMs - head.deferredMs >= timers.maxDeferralSeconds * 1000)
          actions.push({
            kind: "clear_then_send",
            messageId: head.messageId,
            notifyOperator: !head.inputClearRecorded,
          });
      } else if (
        nowMs - head.deferredMs >=
        timers.maxBusyDeferralSeconds * 1000
      )
        transitions.push({ messageId: head.messageId, to: "expired" });
    }
    if (
      agent.kind === "PM" &&
      head !== undefined &&
      (head.state === "queued" ||
        head.state === "sent" ||
        head.state === "unacked")
    ) {
      const counted = countedMillis(
        "ack",
        head.queuedMs,
        nowMs,
        agent.observations,
        agent.waits,
      );
      if (counted >= timers.pmNotifyAfterSeconds * 1000) {
        if (head.lastNotifiedMs === null)
          actions.push({
            kind: "notify_operator",
            messageId: head.messageId,
            repeat: false,
          });
        else if (
          nowMs - head.lastNotifiedMs >=
          timers.notifyIntervalSeconds * 1000
        )
          actions.push({
            kind: "notify_operator",
            messageId: head.messageId,
            repeat: true,
          });
      }
    }
    if (
      agent.kind === "PM" &&
      timers.pmWakeAfterSeconds > 0 &&
      head?.state === "queued" &&
      nowMs - head.queuedMs >= timers.pmWakeAfterSeconds * 1000 &&
      head.wakeCount < MAX_PM_WAKES &&
      (head.lastWakeMs === null ||
        nowMs - head.lastWakeMs >= timers.pmWakeIntervalSeconds * 1000)
    ) {
      const state = stateAt(agent.observations, nowMs);
      if (state === "idle" || state === "done")
        actions.push({ kind: "wake_pm", messageId: head.messageId });
    }
    if (
      agent.kind !== "PM" &&
      agent.kind !== "Supervisor" &&
      stateAt(agent.observations, nowMs) === "blocked"
    ) {
      const since = stateSince(agent.observations, nowMs);
      if (since !== null && nowMs - since >= timers.stallAfterSeconds * 1000)
        attention.push({
          agentId: agent.agentId,
          kind: "blocked",
          episodeMs: since,
        });
    }
    if (stateAt(agent.observations, nowMs) === "working") {
      const stalled =
        countedMillis(
          "stall",
          agent.lastActivityMs,
          nowMs,
          agent.observations,
          agent.waits,
        ) >=
        timers.stallAfterSeconds * 1000;
      if (stalled) {
        stalledAgentIds.push(agent.agentId);
        if (agent.kind !== "PM" && agent.kind !== "Supervisor")
          attention.push({
            agentId: agent.agentId,
            kind: "stalled",
            episodeMs: agent.lastActivityMs,
          });
      }
    }
  }
  return { transitions, actions, stalledAgentIds, attention };
}

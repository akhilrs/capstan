/**
 * The delivery driver: on every tick it observes agents, applies the message
 * timers, notifies the operator and pushes worker messages through the Herdr
 * adapter. It never types into a PM pane and never resends: a message is
 * recorded as sent before it is typed, and a retry is the operator's decision.
 */
import { newContext } from "./context.js";
import type { ControllerCore } from "./controller/core.js";
import {
  MAX_INPUT_CLEAR_BYTES,
  MessageTransitionError,
} from "./controller/core.js";
import {
  queueHead,
  type DeferralReason,
  type HerdrState,
  type MessagingAction,
  type MessagingTimers,
} from "./controller/messaging.js";
import type { AgentRecord, MessageRecord } from "./controller/types.js";
import {
  AgentPaneMismatch,
  ClearFailed,
  DeferralNotElapsed,
  INPUT_UNREADABLE_DETAIL,
  InputUnreadable,
  InvalidArgumentError,
  MAX_TEXT_BYTES,
  NotIdle,
  PhaseError,
  SendAfterRecordError,
  UnknownPaneError,
  isAgentName,
  type KeyLogger,
  type SendOutcome,
} from "./herdr/adapter.js";
import { HerdrError } from "./herdr/runner.js";
import type { DriverSnapshot } from "./commands.js";
import type { Notifier, NotificationRequest } from "./notifier.js";

export const MIN_TICK_MS = 500;
export const DEFAULT_TICK_MS = 2_000;
export const FAILURE_LIMIT = 3;
export const STUCK_AFTER_TICKS = 10;
const TRUNCATION_MARKER = "[truncated]";

/** The part of the Herdr adapter the driver uses; tests stub it. */
export interface DriverAdapter {
  paneForAgent(agentId: string): string | undefined;
  paneEntry(paneId: string): { readonly agent?: string } | undefined;
  agentObservation(agentId: string): Promise<HerdrState>;
  guardedSend(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }): Promise<SendOutcome>;
  clearAfterDeferral(input: {
    paneId: string;
    deferredForMs: number;
    maxDeferralMs: number;
    discard: (text: string) => void | Promise<void>;
    log: KeyLogger;
  }): Promise<{ cleared: boolean; text: string }>;
}

export type DriverLog = (
  event: string,
  details: Record<string, unknown>,
) => void;

export interface DriverOptions {
  readonly core: ControllerCore;
  readonly adapter: DriverAdapter;
  readonly timers: MessagingTimers;
  readonly notifier: Notifier;
  /** The project (operator) credential, which holds controller:reconcile. */
  readonly credential: string;
  readonly now?: () => number;
  readonly log?: DriverLog;
  readonly tickMs?: number;
}

/** Thrown from a callback that must stop the physical action because the ledger moved on. */
export class StaleActionError extends Error {
  override readonly name = "StaleActionError";
}

type SkipReason =
  | "no_pane"
  | "pane_mismatch"
  | "pane_not_ready"
  | "input_unreadable"
  | "herdr_error"
  | "clear_wait";

function truncateForRecord(text: string): string {
  if (Buffer.byteLength(text, "utf8") <= MAX_INPUT_CLEAR_BYTES) return text;
  const budget = MAX_INPUT_CLEAR_BYTES - Buffer.byteLength(TRUNCATION_MARKER);
  let out = "";
  let bytes = 0;
  for (const character of text) {
    const size = Buffer.byteLength(character, "utf8");
    if (bytes + size > budget) break;
    out += character;
    bytes += size;
  }
  return `${out}${TRUNCATION_MARKER}`;
}

export class DeliveryDriver {
  readonly #core: ControllerCore;
  readonly #adapter: DriverAdapter;
  readonly #timers: MessagingTimers;
  readonly #notifier: Notifier;
  readonly #credential: string;
  readonly #now: () => number;
  readonly #log: DriverLog;
  readonly #tickMs: number;
  #running: Promise<void> | undefined;
  #stopped = false;
  #timer: NodeJS.Timeout | undefined;
  #stalled: readonly string[] = [];
  readonly #once = new Set<string>();
  readonly #failures = new Map<string, number>();
  readonly #skipCounts = new Map<
    string,
    { reason: SkipReason; ticks: number }
  >();
  #tickSkips = new Map<string, SkipReason>();
  #skippedAgents = new Set<string>();
  #stuck: { messageId: string; reason: string }[] = [];

  constructor(options: DriverOptions) {
    this.#core = options.core;
    this.#adapter = options.adapter;
    this.#timers = options.timers;
    this.#notifier = options.notifier;
    this.#credential = options.credential;
    this.#now = options.now ?? Date.now;
    this.#log = options.log ?? (() => undefined);
    this.#tickMs = Math.max(options.tickMs ?? DEFAULT_TICK_MS, MIN_TICK_MS);
  }

  snapshot(): DriverSnapshot {
    return { stalledAgentIds: this.#stalled, stuck: this.#stuck };
  }

  start(): void {
    this.#stopped = false;
    const loop = async (): Promise<void> => {
      if (this.#stopped) return;
      try {
        await this.tick();
      } catch (error) {
        this.#log("tick_failed", { error: String(error) });
      }
      if (!this.#stopped)
        this.#timer = setTimeout(() => void loop(), this.#tickMs);
    };
    this.#timer = setTimeout(() => void loop(), 0);
  }

  async stop(): Promise<void> {
    this.#stopped = true;
    clearTimeout(this.#timer);
    await this.#running;
  }

  /** Runs one tick; a call that overlaps a running tick returns at once. */
  async tick(): Promise<void> {
    if (this.#running !== undefined) return;
    this.#running = this.#tick().finally(() => {
      this.#running = undefined;
    });
    await this.#running;
  }

  #context() {
    return newContext(this.#core, this.#credential);
  }

  #logOnce(key: string, event: string, details: Record<string, unknown>): void {
    if (this.#once.has(key)) return;
    this.#once.add(key);
    this.#log(event, details);
  }

  #skip(messageId: string, agentId: string, reason: SkipReason): void {
    this.#tickSkips.set(messageId, reason);
    this.#skippedAgents.add(agentId);
  }

  async #tick(): Promise<void> {
    this.#tickSkips = new Map();
    this.#skippedAgents = new Set();
    const agents = this.#core.listAgents().filter((a) => a.state === "active");
    for (const agent of agents) await this.#observe(agent);
    await this.#advance();
    for (const agent of agents)
      if (agent.kind !== "PM" && !this.#skippedAgents.has(agent.agentId))
        await this.#deliver(agent);
    await this.#updateStuck();
  }

  async #observe(agent: AgentRecord): Promise<void> {
    if (this.#adapter.paneForAgent(agent.agentId) === undefined) return;
    try {
      const state = await this.#adapter.agentObservation(agent.agentId);
      this.#core.recordAgentObservation(this.#context(), agent.agentId, state);
    } catch (error) {
      const name =
        error instanceof AgentPaneMismatch ? "pane_mismatch" : "observe_failed";
      this.#logOnce(`${agent.agentId}|${name}`, name, {
        agentId: agent.agentId,
        error: error instanceof Error ? error.name : "error",
      });
    }
  }

  async #advance(): Promise<void> {
    let advance;
    try {
      advance = this.#core.advanceMessaging(this.#context(), this.#timers);
    } catch (error) {
      this.#log("advance_failed", { error: String(error) });
      return;
    }
    this.#stalled = advance.stalledAgentIds;
    for (const agentId of advance.stalledAgentIds)
      this.#logOnce(`${agentId}|agent_stalled`, "agent_stalled", { agentId });
    for (const action of advance.actions) {
      try {
        if (action.kind === "notify_operator") await this.#notifyHead(action);
        else await this.#clear(action);
      } catch (error) {
        this.#log("action_failed", { kind: action.kind, error: String(error) });
      }
    }
  }

  #isHead(message: MessageRecord): boolean {
    return (
      queueHead(this.#core.messagesFor(message.recipientAgentId))?.messageId ===
      message.messageId
    );
  }

  #isStale(error: unknown, messageId: string): boolean {
    if (error instanceof StaleActionError) {
      this.#log("stale_action", { messageId });
      return true;
    }
    if (error instanceof MessageTransitionError) {
      this.#log("stale_action", { messageId, code: error.code });
      return true;
    }
    return false;
  }

  async #notifyHead(
    action: Extract<MessagingAction, { kind: "notify_operator" }>,
  ): Promise<void> {
    const message = this.#core.message(action.messageId);
    if (message === undefined || !this.#isHead(message)) return;
    const request: NotificationRequest = {
      kind: "pm_message",
      messageId: message.messageId,
      recipientAgentId: message.recipientAgentId,
      repeat: action.repeat,
    };
    const results = await this.#notifier.send(request);
    if (!results.some((r) => r.ok)) {
      this.#notifier.write(request, results, false);
      return;
    }
    let recorded = false;
    try {
      const current = this.#core.message(message.messageId);
      if (current !== undefined && this.#isHead(current)) {
        this.#core.recordNotification(this.#context(), message.messageId);
        recorded = true;
      }
    } catch (error) {
      if (!this.#isStale(error, message.messageId))
        this.#log("record_notification_failed", {
          messageId: message.messageId,
        });
    }
    this.#notifier.write(request, results, recorded);
  }

  #owned(agent: AgentRecord): { paneId: string } | SkipReason {
    if (!isAgentName(agent.agentId)) {
      this.#logOnce(`${agent.agentId}|bad_agent_name`, "bad_agent_name", {
        agentId: agent.agentId,
      });
      return "pane_mismatch";
    }
    const paneId = this.#adapter.paneForAgent(agent.agentId);
    if (paneId === undefined) return "no_pane";
    if (this.#adapter.paneEntry(paneId)?.agent !== agent.agentId)
      return "pane_mismatch";
    return { paneId };
  }

  async #clear(
    action: Extract<MessagingAction, { kind: "clear_then_send" }>,
  ): Promise<void> {
    const message = this.#core.message(action.messageId);
    if (
      message === undefined ||
      message.state !== "deferred" ||
      message.deferredReason !== "input_not_empty" ||
      message.deferredAt === null
    )
      return;
    const agent = this.#core.agentRecord(message.recipientAgentId);
    if (agent === undefined || agent.state !== "active") return;
    const owned = this.#owned(agent);
    if (typeof owned === "string") {
      this.#skip(message.messageId, agent.agentId, owned);
      this.#noteSkip(agent, message, owned);
      return;
    }
    let cleared: { cleared: boolean; text: string } | undefined;
    try {
      cleared = await this.#adapter.clearAfterDeferral({
        paneId: owned.paneId,
        deferredForMs: this.#now() - Date.parse(message.deferredAt),
        maxDeferralMs: this.#timers.maxDeferralSeconds * 1000,
        discard: (text) => {
          const current = this.#core.message(message.messageId);
          if (
            current?.state !== "deferred" ||
            current.deferredReason !== "input_not_empty"
          )
            throw new StaleActionError("the message moved before the clear");
          this.#core.recordInputClear(
            this.#context(),
            message.messageId,
            truncateForRecord(text),
          );
        },
        log: (entry) => this.#log("key", { ...entry }),
      });
    } catch (error) {
      this.#clearFailure(error, agent, message);
      return;
    }
    this.#failures.delete(message.messageId);
    if (cleared.cleared && action.notifyOperator) {
      const request: NotificationRequest = {
        kind: "input_cleared",
        messageId: message.messageId,
        recipientAgentId: message.recipientAgentId,
        repeat: false,
        detail: String([...cleared.text].length),
      };
      this.#notifier.write(request, await this.#notifier.send(request), true);
    }
  }

  #clearFailure(
    error: unknown,
    agent: AgentRecord,
    message: MessageRecord,
  ): void {
    if (this.#isStale(error, message.messageId)) return;
    if (error instanceof DeferralNotElapsed || error instanceof NotIdle) {
      this.#skip(message.messageId, agent.agentId, "clear_wait");
      return;
    }
    if (error instanceof InputUnreadable) {
      this.#logOnce(
        `${message.messageId}|input_unreadable`,
        "input_unreadable",
        {
          messageId: message.messageId,
        },
      );
      this.#skip(message.messageId, agent.agentId, "input_unreadable");
      return;
    }
    if (error instanceof ClearFailed) {
      this.#countFailure(message, "input line could not be cleared");
      this.#skip(message.messageId, agent.agentId, "herdr_error");
      return;
    }
    this.#adapterFailure(error, agent, message);
  }

  #noteSkip(
    agent: AgentRecord,
    message: MessageRecord,
    reason: SkipReason,
  ): void {
    this.#logOnce(`${message.messageId}|${reason}`, reason, {
      agentId: agent.agentId,
      messageId: message.messageId,
    });
  }

  #adapterFailure(
    error: unknown,
    agent: AgentRecord,
    message: MessageRecord,
  ): void {
    if (error instanceof AgentPaneMismatch) {
      this.#skip(message.messageId, agent.agentId, "pane_mismatch");
      this.#noteSkip(agent, message, "pane_mismatch");
      return;
    }
    if (error instanceof PhaseError) {
      this.#skip(message.messageId, agent.agentId, "pane_not_ready");
      this.#logOnce(`${message.messageId}|pane_not_ready`, "pane_not_ready", {
        agentId: agent.agentId,
      });
      return;
    }
    if (error instanceof UnknownPaneError) {
      this.#skip(message.messageId, agent.agentId, "no_pane");
      return;
    }
    if (error instanceof HerdrError) {
      this.#countFailure(message, `Herdr failed: ${error.code}`);
      this.#skip(message.messageId, agent.agentId, "herdr_error");
      return;
    }
    this.#log("delivery_error", {
      messageId: message.messageId,
      error: String(error),
    });
    this.#skip(message.messageId, agent.agentId, "herdr_error");
  }

  #countFailure(message: MessageRecord, reason: string): void {
    const count = (this.#failures.get(message.messageId) ?? 0) + 1;
    this.#failures.set(message.messageId, count);
    this.#log("delivery_failure", { messageId: message.messageId, count });
    if (count < FAILURE_LIMIT) return;
    this.#recordFailure(message.messageId, reason);
  }

  #recordFailure(messageId: string, reason: string): void {
    try {
      const current = this.#core.message(messageId);
      if (
        current === undefined ||
        (current.state !== "queued" && current.state !== "deferred")
      )
        return;
      this.#core.recordFailure(this.#context(), messageId, reason);
      this.#failures.delete(messageId);
    } catch (error) {
      if (!this.#isStale(error, messageId))
        this.#log("record_failure_failed", { messageId });
    }
  }

  async #deliver(agent: AgentRecord): Promise<void> {
    const head = queueHead(this.#core.messagesFor(agent.agentId));
    if (
      head === undefined ||
      (head.state !== "queued" && head.state !== "deferred")
    ) {
      return;
    }
    const sender = this.#core.senderOf(head.senderActorId);
    const from =
      sender.role === "operator"
        ? "operator"
        : sender.agentId === null
          ? sender.role
          : `${sender.role} (${sender.agentId})`;
    const text = `[capstan message ${head.messageId} from ${from}]\n${head.body}\nAcknowledge with: cstan ack ${head.messageId}`;
    if (Buffer.byteLength(text, "utf8") > MAX_TEXT_BYTES) {
      this.#recordFailure(head.messageId, "message too large to type");
      return;
    }
    const owned = this.#owned(agent);
    if (typeof owned === "string") {
      this.#skip(head.messageId, agent.agentId, owned);
      this.#noteSkip(agent, head, owned);
      return;
    }
    let outcome: SendOutcome;
    try {
      outcome = await this.#adapter.guardedSend({
        paneId: owned.paneId,
        text,
        beforeSend: () => {
          const current = this.#core.message(head.messageId);
          if (
            current === undefined ||
            (current.state !== "queued" && current.state !== "deferred") ||
            !this.#isHead(current)
          )
            throw new StaleActionError("the message moved before it was sent");
          this.#core.recordSent(this.#context(), head.messageId);
        },
      });
    } catch (error) {
      if (this.#isStale(error, head.messageId)) return;
      if (error instanceof SendAfterRecordError) {
        this.#failures.delete(head.messageId);
        this.#log("send_after_record", { messageId: head.messageId });
        return;
      }
      if (error instanceof InvalidArgumentError) {
        this.#recordFailure(head.messageId, "text refused by the input rules");
        return;
      }
      this.#adapterFailure(error, agent, head);
      return;
    }
    this.#failures.delete(head.messageId);
    if (outcome.sent) return;
    this.#recordDeferral(head, outcome.reason);
    if (outcome.detail === INPUT_UNREADABLE_DETAIL) {
      this.#skip(head.messageId, agent.agentId, "input_unreadable");
      this.#logOnce(`${head.messageId}|input_unreadable`, "input_unreadable", {
        messageId: head.messageId,
      });
    }
  }

  #recordDeferral(message: MessageRecord, reason: DeferralReason): void {
    try {
      const current = this.#core.message(message.messageId);
      if (
        current === undefined ||
        (current.state !== "queued" && current.state !== "deferred")
      )
        return;
      this.#core.recordDeferral(this.#context(), message.messageId, reason);
    } catch (error) {
      if (!this.#isStale(error, message.messageId))
        this.#log("record_deferral_failed", { messageId: message.messageId });
    }
  }

  async #updateStuck(): Promise<void> {
    for (const [messageId, reason] of this.#tickSkips) {
      const previous = this.#skipCounts.get(messageId);
      this.#skipCounts.set(messageId, {
        reason,
        ticks: previous?.reason === reason ? previous.ticks + 1 : 1,
      });
    }
    for (const messageId of [...this.#skipCounts.keys()])
      if (!this.#tickSkips.has(messageId)) this.#skipCounts.delete(messageId);
    const stuck: { messageId: string; reason: string }[] = [];
    for (const [messageId, { reason, ticks }] of this.#skipCounts) {
      if (
        ticks < STUCK_AFTER_TICKS ||
        reason === "no_pane" ||
        reason === "clear_wait"
      )
        continue;
      stuck.push({ messageId, reason });
      const key = `${messageId}|delivery_stuck|${reason}`;
      if (this.#once.has(key)) continue;
      this.#once.add(key);
      const message = this.#core.message(messageId);
      if (message === undefined) continue;
      const request: NotificationRequest = {
        kind: "delivery_stuck",
        messageId,
        recipientAgentId: message.recipientAgentId,
        repeat: false,
        detail: reason,
      };
      this.#notifier.write(request, await this.#notifier.send(request), false);
    }
    this.#stuck = stuck;
  }
}

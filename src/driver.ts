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
  messagingTimersOf,
  pmWakeBackoffSeconds,
  queueHead,
  type DeferralReason,
  type HerdrState,
  type MessagingAction,
  suppressActiveStalls,
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
import {
  ProcessActivityTracker,
  type ProcessActivityProbe,
} from "./herdr/process-activity.js";
import { HerdrError } from "./herdr/runner.js";
import type { DriverSnapshot } from "./commands.js";
import { DEFAULT_PM_STALE_MINUTES } from "./config/capstan-config.js";
import { pmMailSummary } from "./pm-mail.js";
import type { Notifier, NotificationRequest } from "./notifier.js";

export const MIN_TICK_MS = 500;
export const DEFAULT_TICK_MS = 2_000;
export const FAILURE_LIMIT = 3;
/** Observations in a row that fail with a not-found error before an agent is recorded lost. */
export const LOSS_LIMIT = 3;
/** Ticks in a row in which every observed agent is not found (a Herdr restart?) that the counters are held before they start again from zero. */
export const SUPPRESS_LIMIT = 10;
export const STUCK_AFTER_TICKS = 10;
/** The least time between two process samples of one working agent. */
export const PROCESS_SAMPLE_MS = 15_000;
const PENDING_STATES: readonly string[] = [
  "queued",
  "deferred",
  "sent",
  "unacked",
];
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
  wakePm(input: {
    paneId: string;
    text: string;
    beforeSend: () => void | Promise<void>;
  }): Promise<
    | { readonly sent: true }
    | {
        readonly sent: false;
        readonly reason: "pm_not_idle" | "input_not_empty";
      }
  >;
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
  /** Tells whether a tool process under a working agent uses CPU; such an agent is not reported as stalled. */
  readonly processProbe?: ProcessActivityProbe;
  /** A PM message pending this long raises one stale notification per episode; default DEFAULT_PM_STALE_MINUTES. */
  readonly pmStaleSeconds?: number;
}

/** Thrown from a callback that must stop the physical action because the ledger moved on. */
export class StaleActionError extends Error {
  override readonly name = "StaleActionError";
}

/** The line typed into an idle PM that has an unread message. */
export const PM_WAKE_TEXT = "Run cstan inbox: a teammate has written to you.";

type Outcome = "ok" | "not_found" | "other";

/** The Herdr error codes that mean the agent or its pane is gone (a missing session, workspace or tab is not). */
const NOT_FOUND_CODES: ReadonlySet<string> = new Set([
  "agent_not_found",
  "pane_not_found",
]);

type SkipReason =
  | "no_pane"
  | "pane_mismatch"
  | "pane_not_ready"
  | "input_unreadable"
  | "herdr_error"
  | "clear_wait";

function truncateForRecord(input: string): string {
  // The core refuses text that is not well-formed UTF-16.
  const text = input.toWellFormed();
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
  /** Once-keys whose condition occurred in the current tick; the rest are forgotten so a recurrence is reported again. */
  #seen = new Set<string>();
  readonly #failures = new Map<string, number>();
  readonly #skipCounts = new Map<
    string,
    { reason: SkipReason; ticks: number }
  >();
  #tickSkips = new Map<string, SkipReason>();
  #skippedAgents = new Set<string>();
  #stuck: { messageId: string; reason: string }[] = [];
  /** Consecutive not-found observations per agent. */
  readonly #missing = new Map<string, number>();
  readonly #lost = new Set<string>();
  #suppressedTicks = 0;
  /** After the hold limit, an all-not-found tick counts again until some agent answers. */
  #suppressionSpent = false;
  readonly #processProbe: ProcessActivityProbe | undefined;
  readonly #activity = new ProcessActivityTracker();
  readonly #lastSampleMs = new Map<string, number>();
  readonly #probeFailed = new Set<string>();
  readonly #pmStaleSeconds: number;
  /** The last wake typed for each message this process woke the PM for. */
  readonly #wakes = new Map<
    string,
    { wakeAt: number; count: number; unansweredLogged: boolean }
  >();
  /** The open stale episode by PM, keyed in memory only: a restart during an episode can notify once more. */
  readonly #staleEpisodes = new Map<
    string,
    {
      since: string;
      oldestMessageId: string;
      pending: number;
      notified: boolean;
    }
  >();

  constructor(options: DriverOptions) {
    this.#core = options.core;
    this.#adapter = options.adapter;
    this.#timers = messagingTimersOf(options.timers);
    this.#notifier = options.notifier;
    this.#credential = options.credential;
    this.#now = options.now ?? Date.now;
    this.#log = options.log ?? (() => undefined);
    this.#tickMs = Math.max(options.tickMs ?? DEFAULT_TICK_MS, MIN_TICK_MS);
    this.#processProbe = options.processProbe;
    this.#pmStaleSeconds =
      options.pmStaleSeconds ?? DEFAULT_PM_STALE_MINUTES * 60;
  }

  snapshot(): DriverSnapshot {
    const stale = [...this.#staleEpisodes.values()][0];
    return {
      stalledAgentIds: this.#stalled,
      stuck: this.#stuck,
      lostAgentIds: [...this.#lost].sort(),
      pmStale: stale === undefined ? null : { ...stale },
    };
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
    this.#seen.add(key);
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
    this.#seen = new Set();
    const agents = this.#core.listAgents().filter((a) => a.state === "active");
    const outcomes = new Map<string, Outcome>();
    for (const agent of agents) await this.#observe(agent, outcomes);
    this.#judgeLoss(agents, outcomes);
    this.#forgetProcesses(agents);
    // Wakes are judged before the next one can be typed, so a wake that went unanswered is logged before it is repeated.
    this.#judgeWakes(agents);
    await this.#advance();
    await this.#judgeStale(agents);
    for (const agent of agents)
      if (
        agent.kind !== "PM" &&
        !this.#skippedAgents.has(agent.agentId) &&
        // A paused agent's head stays queued or deferred: no send, no new deferral.
        !this.#core.isDeliveryPaused(agent.agentId)
      )
        await this.#deliver(agent);
    await this.#updateStuck();
    this.#forget();
  }

  async #observe(
    agent: AgentRecord,
    outcomes: Map<string, Outcome>,
  ): Promise<void> {
    if (this.#adapter.paneForAgent(agent.agentId) === undefined) {
      // No pane to look at: a count left from before does not carry over to a pane that comes back.
      this.#missing.delete(agent.agentId);
      return;
    }
    try {
      const state = await this.#adapter.agentObservation(agent.agentId);
      this.#core.recordAgentObservation(this.#context(), agent.agentId, state);
      outcomes.set(agent.agentId, "ok");
      if (state === "working") await this.#sampleProcesses(agent);
    } catch (error) {
      outcomes.set(
        agent.agentId,
        error instanceof HerdrError && NOT_FOUND_CODES.has(error.code)
          ? "not_found"
          : "other",
      );
      const name =
        error instanceof AgentPaneMismatch ? "pane_mismatch" : "observe_failed";
      this.#logOnce(`${agent.agentId}|${name}`, name, {
        agentId: agent.agentId,
        error: error instanceof Error ? error.name : "error",
      });
    }
  }

  /** Samples the tool processes under a working, unpaused agent, at most once per PROCESS_SAMPLE_MS. A failing probe suppresses nothing. */
  async #sampleProcesses(agent: AgentRecord): Promise<void> {
    const probe = this.#processProbe;
    if (probe === undefined || this.#core.isDeliveryPaused(agent.agentId))
      return;
    const paneId = this.#adapter.paneForAgent(agent.agentId);
    if (paneId === undefined) return;
    const now = this.#now();
    const last = this.#lastSampleMs.get(agent.agentId);
    if (last !== undefined && now - last < PROCESS_SAMPLE_MS) return;
    this.#lastSampleMs.set(agent.agentId, now);
    try {
      this.#activity.record(
        agent.agentId,
        await probe.sample(paneId),
        this.#now(),
      );
    } catch (error) {
      if (this.#probeFailed.has(agent.agentId)) return;
      this.#probeFailed.add(agent.agentId);
      this.#log("process_probe_failed", {
        agentId: agent.agentId,
        error: String(error),
      });
    }
  }

  /** Drops the process state of agents that are no longer active or are recorded lost. */
  #forgetProcesses(agents: readonly AgentRecord[]): void {
    const live = new Set(
      agents.map((a) => a.agentId).filter((id) => !this.#lost.has(id)),
    );
    for (const id of [...this.#lastSampleMs.keys()])
      if (!live.has(id)) {
        this.#lastSampleMs.delete(id);
        this.#probeFailed.delete(id);
        this.#activity.forget(id);
      }
  }

  /**
   * Counts consecutive not-found observations. A tick in which every observed
   * agent (at least two) is not found counts for nothing: a Herdr server that
   * restarted would otherwise make every live agent look dead. The counters are
   * held for a few such ticks and then start again from zero.
   */
  #judgeLoss(
    agents: readonly AgentRecord[],
    outcomes: ReadonlyMap<string, Outcome>,
  ): void {
    const active = new Set(agents.map((a) => a.agentId));
    for (const id of [...this.#missing.keys()])
      if (!active.has(id)) this.#missing.delete(id);
    for (const id of [...this.#lost])
      if (!active.has(id)) this.#lost.delete(id);
    const observed = [...outcomes.keys()];
    const notFound = observed.filter((id) => outcomes.get(id) === "not_found");
    const allGone = observed.length >= 2 && notFound.length === observed.length;
    if (allGone && !this.#suppressionSpent) {
      this.#suppressedTicks += 1;
      this.#logOnce("loss_suppressed", "loss_suppressed", {
        agents: observed.length,
      });
      if (this.#suppressedTicks >= SUPPRESS_LIMIT) {
        this.#missing.clear();
        this.#suppressedTicks = 0;
        this.#suppressionSpent = true;
      }
      return;
    }
    if (!allGone) {
      this.#suppressedTicks = 0;
      this.#suppressionSpent = false;
    }
    for (const id of observed) {
      const outcome = outcomes.get(id);
      if (outcome === "ok") {
        this.#missing.delete(id);
        this.#lost.delete(id);
      } else if (outcome === "other") {
        // Not knowing is not seeing it gone: the row of not-found observations is broken.
        this.#missing.delete(id);
      } else if (outcome === "not_found") {
        const count = (this.#missing.get(id) ?? 0) + 1;
        this.#missing.set(id, count);
        if (count >= LOSS_LIMIT && !this.#lost.has(id)) this.#markLost(id);
      }
    }
  }

  /** Listed lost only once the ledger took it, so a failed write is tried again on the next tick. */
  #markLost(agentId: string): void {
    try {
      const { recorded } = this.#core.recordAgentLost(this.#context(), {
        agentId,
      });
      this.#lost.add(agentId);
      this.#log("agent_lost", { agentId, recorded });
    } catch (error) {
      this.#logOnce(
        `${agentId}|agent_lost_not_recorded`,
        "agent_lost_not_recorded",
        {
          agentId,
          error: String(error),
        },
      );
    }
  }

  /** Open unreadable-input episodes by agent: the deferred head message that opened each; closed when the input is read. */
  readonly #unreadable = new Map<string, string>();

  #openUnreadable(
    agent: AgentRecord,
    message: MessageRecord,
    blocker: unknown,
  ): void {
    if (this.#unreadable.has(agent.agentId)) return;
    this.#unreadable.set(agent.agentId, message.messageId);
    try {
      this.#core.queueInputBlockedNotice(this.#context(), {
        agentId: agent.agentId,
        messageId: message.messageId,
        blocker:
          blocker === "dialog" || blocker === "permission_prompt"
            ? blocker
            : "unknown",
      });
    } catch (error) {
      this.#log("attention_notice_failed", { error: String(error) });
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
    const reported = advance.stalledAgentIds;
    advance = suppressActiveStalls(
      advance,
      this.#activity.activity(),
      this.#now(),
      this.#timers,
    );
    for (const agentId of reported)
      if (!advance.stalledAgentIds.includes(agentId))
        this.#logOnce(`${agentId}|stall_suppressed`, "stall_suppressed", {
          agentId,
        });
    this.#stalled = advance.stalledAgentIds;
    for (const agentId of advance.stalledAgentIds)
      this.#logOnce(`${agentId}|agent_stalled`, "agent_stalled", { agentId });
    try {
      this.#core.queueMissingDeliveryNotices(this.#context());
    } catch (error) {
      this.#log("delivery_notice_failed", { error: String(error) });
    }
    if (advance.attention.length > 0) {
      try {
        this.#core.queueAttentionNotices(this.#context(), advance.attention);
      } catch (error) {
        this.#log("attention_notice_failed", { error: String(error) });
      }
    }
    for (const action of advance.actions) {
      try {
        if (action.kind === "notify_operator") await this.#notifyHead(action);
        else if (action.kind === "wake_pm") await this.#wake(action);
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

  /** Types one wake line into an idle PM that has an unread message; the wake is recorded first and never repeated sooner than the backoff. */
  async #wake(
    action: Extract<MessagingAction, { kind: "wake_pm" }>,
  ): Promise<void> {
    const message = this.#core.message(action.messageId);
    if (message === undefined || message.state !== "queued") return;
    const agent = this.#core.agentRecord(message.recipientAgentId);
    if (agent === undefined || agent.state !== "active" || agent.kind !== "PM")
      return;
    const owned = this.#owned(agent);
    if (typeof owned === "string") return;
    let recorded = false;
    try {
      const outcome = await this.#adapter.wakePm({
        paneId: owned.paneId,
        text: PM_WAKE_TEXT,
        beforeSend: () => {
          const current = this.#core.message(message.messageId);
          if (current === undefined || current.state !== "queued")
            throw new StaleActionError("the message is no longer unread");
          this.#core.recordPmWake(this.#context(), message.messageId);
          recorded = true;
        },
      });
      if (!outcome.sent)
        this.#logOnce(
          `${message.messageId}|wake_skipped|${outcome.reason}`,
          "wake_skipped",
          { messageId: message.messageId, reason: outcome.reason },
        );
      else {
        const previous = this.#wakes.get(message.messageId);
        this.#wakes.set(message.messageId, {
          wakeAt: this.#now(),
          count: (previous?.count ?? 0) + 1,
          unansweredLogged: false,
        });
        this.#log("pm_wake_typed", { messageId: message.messageId });
      }
    } catch (error) {
      if (recorded) {
        // The wake is in the ledger, so the next one waits for the backoff; it was not typed, so it is not a wake.
        this.#log("pm_wake_failed", {
          messageId: message.messageId,
          error: String(error),
        });
      } else if (!this.#isStale(error, message.messageId))
        this.#log("wake_failed", {
          messageId: message.messageId,
          error: String(error),
        });
    }
  }

  /** A wake is answered when the message left the queued state or the PM pulled or acknowledged any message after it; one still queued after the next backoff step is logged once. */
  #judgeWakes(agents: readonly AgentRecord[]): void {
    const messages = agents
      .filter((a) => a.kind === "PM")
      .flatMap((pm) => this.#pmMessages(pm));
    const now = this.#now();
    for (const [messageId, wake] of [...this.#wakes]) {
      const message = messages.find((m) => m.messageId === messageId);
      const pulledSince = messages.some((m) =>
        [m.sentAt, m.ackedAt].some(
          (at) => at !== null && Date.parse(at) > wake.wakeAt,
        ),
      );
      if (message === undefined || message.state !== "queued" || pulledSince) {
        this.#wakes.delete(messageId);
        continue;
      }
      const intervalMs =
        pmWakeBackoffSeconds(this.#timers.pmWakeIntervalSeconds, wake.count) *
        1000;
      if (!wake.unansweredLogged && now - wake.wakeAt >= intervalMs) {
        wake.unansweredLogged = true;
        this.#log("pm_wake_unanswered", { messageId, wakes: wake.count });
      }
    }
  }

  /** One stale notification per episode; an episode is keyed by the oldest pending message and ends when that message is no longer pending. */
  async #judgeStale(agents: readonly AgentRecord[]): Promise<void> {
    const pms = agents.filter((a) => a.kind === "PM");
    for (const pm of pms) await this.#judgeStaleFor(pm);
    const live = new Set(pms.map((a) => a.agentId));
    for (const id of [...this.#staleEpisodes.keys()])
      if (!live.has(id)) this.#staleEpisodes.delete(id);
  }

  #pmMessages(pm: AgentRecord): readonly MessageRecord[] {
    try {
      return this.#core.messagesFor(pm.agentId);
    } catch (error) {
      this.#log("pm_mail_failed", { error: String(error) });
      return [];
    }
  }

  async #judgeStaleFor(pm: AgentRecord): Promise<void> {
    const messages = this.#pmMessages(pm);
    const now = this.#now();
    const summary = pmMailSummary(messages, now, this.#pmStaleSeconds);
    const episode = this.#staleEpisodes.get(pm.agentId);
    const stillPending =
      episode !== undefined &&
      messages.some(
        (m) =>
          m.messageId === episode.oldestMessageId &&
          PENDING_STATES.includes(m.state),
      );
    if (episode !== undefined && !stillPending)
      this.#staleEpisodes.delete(pm.agentId);
    if (!summary.stale || summary.oldestMessageId === null) return;
    let current = this.#staleEpisodes.get(pm.agentId);
    if (current === undefined) {
      current = {
        since: new Date(now).toISOString(),
        oldestMessageId: summary.oldestMessageId,
        pending: summary.pending,
        notified: false,
      };
      this.#staleEpisodes.set(pm.agentId, current);
    }
    current.pending = summary.pending;
    if (current.notified || this.#core.isDeliveryPaused(pm.agentId)) return;
    current.notified = true;
    const request: NotificationRequest = {
      kind: "pm_stale",
      messageId: current.oldestMessageId,
      recipientAgentId: pm.agentId,
      repeat: false,
      detail: `${summary.pending} message${summary.pending === 1 ? "" : "s"}`,
    };
    try {
      this.#notifier.write(request, await this.#notifier.send(request), false);
    } catch (error) {
      this.#log("pm_stale_notify_failed", { error: String(error) });
    }
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
    this.#unreadable.delete(agent.agentId);
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
      this.#openUnreadable(
        agent,
        message,
        (error as { blocker?: unknown }).blocker,
      );
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
    if (error instanceof TypeError) {
      // The core refused the text it was asked to record; retrying the same
      // text can only fail the same way, so this counts toward the limit.
      this.#countFailure(message, "the typed text could not be recorded");
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
    if (outcome.sent) {
      this.#unreadable.delete(agent.agentId);
      return;
    }
    this.#recordDeferral(head, outcome.reason);
    if (
      outcome.reason === "input_not_empty" &&
      outcome.detail !== INPUT_UNREADABLE_DETAIL
    )
      this.#unreadable.delete(agent.agentId);
    if (outcome.detail === INPUT_UNREADABLE_DETAIL) {
      this.#openUnreadable(
        agent,
        head,
        (outcome as { blocker?: unknown }).blocker,
      );
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

  /** Drops once-keys whose condition did not occur this tick and failure counts of messages that left the delivery queue. */
  #forget(): void {
    for (const key of [...this.#once])
      if (!this.#seen.has(key)) this.#once.delete(key);
    for (const messageId of [...this.#failures.keys()]) {
      const state = this.#core.message(messageId)?.state;
      if (state !== "queued" && state !== "deferred")
        this.#failures.delete(messageId);
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
      this.#seen.add(key);
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

import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import { type AuthenticatedActor } from "./auth.js";
import { sha256 } from "./canonical.js";
import {
  evaluateMessaging,
  isFinalState,
  queueHead,
  type AgentFacts,
  type AttentionEpisode,
  type HerdrState,
  type MessageFacts,
  type MessageState,
  type MessagingEvaluation,
  type MessagingTimers,
} from "./messaging.js";
import {
  type InputBlocker,
  type MessageRecord,
  type MessagingAdvance,
  type MutationContext,
} from "./types.js";
import { ControllerError, NoMessageTransitionDue } from "./errors.js";
import {
  SUPERVISION_CHECK_TEXT,
  type MessageRejection,
  type AgentRow,
  type MessageRow,
  type MutationOutput,
} from "./records.js";
import {
  safeId,
  assertTimers,
  oneLineText,
  messageRecord,
  advance,
} from "./helpers.js";

export class MessageNoticesArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  /** The controller's own actor and the one active PM, or undefined when either is missing. */
  noticeParties(): { controllerActorId: string; pm: AgentRow } | undefined {
    const pms = this.kernel.database
      .prepare(
        "SELECT * FROM agents WHERE project_id = ? AND kind = 'PM' AND state = 'active'",
      )
      .all(this.kernel.projectId) as AgentRow[];
    const controller = this.kernel.database
      .prepare(
        "SELECT actor_id FROM actors WHERE project_id = ? AND is_internal = 1 AND role = 'controller' AND active = 1 AND revoked_at IS NULL",
      )
      .get(this.kernel.projectId) as { actor_id: string } | undefined;
    if (pms.length !== 1 || controller === undefined) return undefined;
    return { controllerActorId: controller.actor_id, pm: pms[0]! };
  }

  /** Queues a controller notice to the one active PM; false when there is none, and then the reconcile loop re-sends it (unannouncedPlanNotices). The caller owns the transaction. */
  noticeToPm(body: string, now: string): boolean {
    const parties = this.noticeParties();
    if (parties === undefined) return false;
    this.areas.messages.insertQueuedMessage(
      parties.controllerActorId,
      parties.pm,
      body,
      sha256(body),
      now,
    );
    return true;
  }

  /** Wait rows that are still open; only a controller may list them. */
  openWaits(
    credential: string,
  ): readonly { readonly waitId: string; readonly agentId: string }[] {
    this.kernel.authorize(credential, "controller:reconcile");
    return (
      this.kernel.database
        .prepare(
          "SELECT wait_id, agent_id FROM agent_waits WHERE project_id = ? AND ended_at IS NULL ORDER BY started_at, wait_id",
        )
        .all(this.kernel.projectId) as Array<{
        wait_id: string;
        agent_id: string;
      }>
    ).map((row) => ({ waitId: row.wait_id, agentId: row.agent_id }));
  }

  recordNotification(
    context: MutationContext,
    messageId: string,
  ): MessageRecord {
    safeId(messageId, "message id");
    return this.areas.messages.messageMutation(
      context,
      "message.notified",
      "controller:reconcile",
      { messageId },
      (actor) => {
        const row = this.areas.messages.messageRow(messageId);
        const head =
          row === undefined
            ? undefined
            : queueHead(
                this.areas.messages.messageRowsFor(row.recipient_agent_id),
              );
        if (
          row === undefined ||
          head?.message_id !== row.message_id ||
          isFinalState(row.state)
        )
          return this.areas.messages.reject(actor, "message.notified", {
            messageId,
            code: "not_notifiable",
            message: "only the head of a queue that is not final is notified",
            fromState: row?.state,
          });
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            "UPDATE messages SET notified_at = COALESCE(notified_at, ?), last_notified_at = ?, updated_at = ? WHERE project_id = ? AND message_id = ?",
          )
          .run(now, now, now, this.kernel.projectId, messageId);
        return {
          value: messageRecord(this.areas.messages.messageRow(messageId)!),
          event: {
            entityType: "message_note",
            entityId: messageId,
            stateVersion: row.state_version,
            fromState: row.state,
            toState: row.state,
            details: { notified: true },
          },
        };
      },
    );
  }

  beginWait(context: MutationContext): { readonly waitId: string } {
    return this.areas.messages.messageMutation(
      context,
      "wait.begin",
      "message:receive",
      {},
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        if (agent === undefined)
          return this.areas.messages.reject(actor, "wait.begin", {
            code: "not_an_agent",
            message: "the caller is not the current actor of an active agent",
          });
        const waitId = randomUUID();
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            "INSERT INTO agent_waits(project_id, wait_id, agent_id, started_at) VALUES (?, ?, ?, ?)",
          )
          .run(this.kernel.projectId, waitId, agent.agent_id, now);
        this.kernel.touchAgent(agent.agent_id, now);
        return {
          value: { waitId },
          event: {
            entityType: "agent_wait",
            entityId: waitId,
            stateVersion: 0,
            toState: "open",
            details: { agentId: agent.agent_id },
          },
        };
      },
    );
  }

  endWait(
    context: MutationContext,
    waitId: string,
  ): { readonly ended: boolean } {
    safeId(waitId, "wait id");
    const caller = this.kernel.authorize(context.credential, "message:receive");
    if (
      !this.kernel.hasStoredRequest(context) &&
      this.#waitAlreadyClosedFor(caller, waitId)
    )
      return { ended: false };
    return this.areas.messages.messageMutation(
      context,
      "wait.end",
      "message:receive",
      { waitId },
      (actor) => this.#endWait(actor, "wait.end", waitId),
    );
  }

  endWaitAsController(
    context: MutationContext,
    waitId: string,
  ): { readonly ended: boolean } {
    safeId(waitId, "wait id");
    const caller = this.kernel.authorize(
      context.credential,
      "controller:reconcile",
    );
    if (
      !this.kernel.hasStoredRequest(context) &&
      this.#waitAlreadyClosedFor(caller, waitId)
    )
      return { ended: false };
    return this.areas.messages.messageMutation(
      context,
      "wait.end_controller",
      "controller:reconcile",
      { waitId },
      (actor) => this.#endWait(actor, "wait.end_controller", waitId),
    );
  }

  advanceMessaging(
    context: MutationContext,
    timers: MessagingTimers,
  ): MessagingAdvance {
    assertTimers(timers);
    this.kernel.authorize(context.credential, "controller:reconcile");
    const first = this.#evaluateMessaging(timers);
    if (
      first.transitions.length === 0 &&
      !this.kernel.hasStoredRequest(context)
    )
      return advance(first, []);
    try {
      return this.kernel.mutate(
        context,
        "message.advance",
        "controller:reconcile",
        { timers },
        (actor) => {
          const evaluation = this.#evaluateMessaging(timers);
          if (evaluation.transitions.length === 0)
            throw new NoMessageTransitionDue();
          const now = this.kernel.now();
          const applied: string[] = [];
          for (const transition of evaluation.transitions) {
            const row = this.areas.messages.messageRow(transition.messageId);
            const from: MessageState =
              transition.to === "unacked" ? "sent" : "deferred";
            if (row?.state !== from) continue;
            const version = this.areas.messages.updateMessage(
              row,
              transition.to,
              {},
              now,
            );
            this.areas.messages.appendMessageEvent(actor, context, {
              messageId: row.message_id,
              from,
              to: transition.to,
              stateVersion: version,
              details: { timer: true },
            });
            this.queueDeliveryNotice(row, transition.to, now);
            applied.push(row.message_id);
          }
          return {
            value: advance(evaluation, applied),
            event: {
              entityType: "message_timer",
              entityId: this.kernel.projectId,
              stateVersion: 0,
              details: {
                transitions: applied,
                actions: evaluation.actions.length,
              },
            },
          };
        },
      );
    } catch (error) {
      if (error instanceof NoMessageTransitionDue) return advance(first, []);
      throw error;
    }
  }

  /** Queues one controller message to the PM, once per (kind, subject, episode). The caller owns the transaction. */
  #queuePmNotice(
    kind: "delivery" | "stalled" | "blocked",
    subject: string,
    episode: string,
    body: string,
    now: string,
  ): boolean {
    const parties = this.noticeParties();
    if (parties === undefined) return false;
    const known = this.kernel.database
      .prepare(
        "SELECT 1 AS present FROM pm_notices WHERE project_id = ? AND kind = ? AND subject = ? AND episode = ?",
      )
      .get(this.kernel.projectId, kind, subject, episode);
    if (known) return false;
    const messageId = this.areas.messages.insertQueuedMessage(
      parties.controllerActorId,
      parties.pm,
      body,
      sha256(body),
      now,
    );
    this.kernel.database
      .prepare(
        "INSERT INTO pm_notices(project_id, notice_id, kind, subject, episode, message_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      )
      .run(
        this.kernel.projectId,
        randomUUID(),
        kind,
        subject,
        episode,
        messageId,
        now,
      );
    return true;
  }

  /** Tells the PM that a message to a worker is stuck, and which command unblocks the worker's queue. Routine Supervisor checks and messages to the PM are not told. */
  queueDeliveryNotice(
    row: MessageRow,
    state: "unacked" | "expired" | "failed",
    now: string,
    reason: string | null = null,
  ): void {
    const recipient = this.kernel.agentRow(row.recipient_agent_id);
    if (recipient === undefined || recipient.kind === "PM") return;
    const routine = this.kernel.database
      .prepare(
        "SELECT 1 AS present FROM supervision_checks WHERE project_id = ? AND message_id = ?",
      )
      .get(this.kernel.projectId, row.message_id);
    if (routine) return;
    const first = oneLineText(row.body, 80);
    // One notice per delivery attempt: a message retried with `cstan resolve ... retry` that goes wrong again is told again.
    this.#queuePmNotice(
      "delivery",
      row.message_id,
      `${state}#${row.send_attempts}`,
      [
        `Delivery problem: message ${row.message_id} to ${recipient.agent_id} is ${state}${reason === null ? "" : ` (${oneLineText(reason, 120)})`}.`,
        `It starts: ${JSON.stringify(first)}`,
        `Messages behind it wait for ${recipient.agent_id} until you resolve it: cstan resolve ${row.message_id} retry (types it once more), skip (counts it handled) or cancel (drops it).`,
      ].join("\n"),
      now,
    );
  }

  /**
   * Tells the PM about stuck worker messages that have no notice yet: those that
   * went wrong while no PM was active, and any that were stuck before the
   * notices existed. Nothing is written while there is none.
   */
  queueMissingDeliveryNotices(context: MutationContext): {
    readonly queued: number;
  } {
    this.kernel.authorize(context.credential, "controller:reconcile");
    const missing = (): MessageRow[] =>
      (
        this.kernel.database
          .prepare(
            `SELECT m.* FROM messages m JOIN agents a ON a.project_id = m.project_id AND a.agent_id = m.recipient_agent_id
             WHERE m.project_id = ? AND a.state = 'active' AND a.kind <> 'PM'
               AND m.state IN ('unacked', 'expired', 'failed')
               AND NOT EXISTS (SELECT 1 FROM pm_notices n WHERE n.project_id = m.project_id AND n.kind = 'delivery'
                               AND n.subject = m.message_id AND n.episode = m.state || '#' || m.send_attempts)
               AND NOT EXISTS (SELECT 1 FROM supervision_checks c WHERE c.project_id = m.project_id AND c.message_id = m.message_id)
             ORDER BY m.sequence`,
          )
          .all(this.kernel.projectId) as MessageRow[]
      ).slice(0, 20);
    if (this.noticeParties() === undefined || missing().length === 0)
      return { queued: 0 };
    return this.kernel.mutate(
      context,
      "pm.delivery_notices",
      "controller:reconcile",
      {},
      () => {
        const now = this.kernel.now();
        let queued = 0;
        for (const row of missing()) {
          const before = this.kernel.database
            .prepare(
              "SELECT COUNT(*) AS n FROM pm_notices WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { n: number };
          this.queueDeliveryNotice(
            row,
            row.state as "unacked" | "expired" | "failed",
            now,
            row.state === "failed" ? row.state_reason : null,
          );
          const after = this.kernel.database
            .prepare(
              "SELECT COUNT(*) AS n FROM pm_notices WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { n: number };
          if (after.n > before.n) queued += 1;
        }
        return {
          value: { queued },
          event: {
            entityType: "pm_notice",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: { queued, backfill: true },
          },
        };
      },
    );
  }

  /** True while a notice for an unreadable input of this agent names a head message that is still deferred or queued. */
  #hasOpenUnreadableNotice(agentId: string): boolean {
    const row = this.kernel.database
      .prepare(
        `SELECT 1 AS present FROM pm_notices n JOIN messages m ON m.project_id = n.project_id AND m.message_id = substr(n.episode, 12)
         WHERE n.project_id = ? AND n.kind = 'blocked' AND n.subject = ? AND n.episode LIKE 'unreadable:%'
           AND m.state IN ('deferred', 'queued') LIMIT 1`,
      )
      .get(this.kernel.projectId, agentId);
    return row !== undefined;
  }

  /** Tells the PM, once per deferred head message, that a dialog covers an agent's input box so messages to it wait. */
  queueInputBlockedNotice(
    context: MutationContext,
    input: {
      readonly agentId: string;
      readonly messageId: string;
      readonly blocker: InputBlocker;
    },
  ): { readonly queued: boolean } {
    this.kernel.authorize(context.credential, "controller:reconcile");
    safeId(input.agentId, "agent id");
    safeId(input.messageId, "message id");
    const episode = `unreadable:${input.messageId}`;
    const known = this.kernel.database
      .prepare(
        "SELECT 1 AS present FROM pm_notices WHERE project_id = ? AND kind = 'blocked' AND subject = ? AND episode = ?",
      )
      .get(this.kernel.projectId, input.agentId, episode);
    if (known || this.noticeParties() === undefined) return { queued: false };
    const relay = this.areas.promptRelay.enabled;
    const advice =
      relay && input.blocker === "dialog"
        ? `Run cstan prompt show ${input.agentId}: it can relay this dialog with Esc only.`
        : relay && input.blocker === "permission_prompt"
          ? `Run cstan prompt show ${input.agentId}: it can relay this permission prompt.`
          : `cstan prompt show cannot relay it; look with cstan observe ${input.agentId} and tell the operator.`;
    const body = `Agent blocked: ${input.agentId} cannot take messages because a dialog covers its input box; messages to it wait until it is cleared.\n${advice}`;
    return this.kernel.mutate(
      context,
      "pm.input_blocked",
      "controller:reconcile",
      { ...input },
      () => {
        const queued = this.#queuePmNotice(
          "blocked",
          input.agentId,
          episode,
          body,
          this.kernel.now(),
        );
        return {
          value: { queued },
          event: {
            entityType: "pm_notice",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: { queued, blocker: input.blocker },
          },
        };
      },
    );
  }

  /** Tells the PM, once per episode, of workers that have been stalled (working without activity) or blocked at a dialog for the stall time. */
  queueAttentionNotices(
    context: MutationContext,
    episodes: readonly AttentionEpisode[],
  ): { readonly queued: number } {
    this.kernel.authorize(context.credential, "controller:reconcile");
    const fresh = episodes.filter((episode) => {
      if (
        episode.kind === "blocked" &&
        this.#hasOpenUnreadableNotice(episode.agentId)
      )
        return false;
      const known = this.kernel.database
        .prepare(
          "SELECT 1 AS present FROM pm_notices WHERE project_id = ? AND kind = ? AND subject = ? AND episode = ?",
        )
        .get(
          this.kernel.projectId,
          episode.kind,
          episode.agentId,
          String(episode.episodeMs),
        );
      return !known;
    });
    if (fresh.length === 0) return { queued: 0 };
    return this.kernel.mutate(
      context,
      "pm.attention",
      "controller:reconcile",
      { episodes: fresh },
      () => {
        const now = this.kernel.now();
        let queued = 0;
        for (const episode of fresh) {
          const body =
            episode.kind === "stalled"
              ? `Agent stalled: ${episode.agentId} has shown no activity while working for a long time. Look with cstan observe ${episode.agentId}; if it is stuck, cstan replace ${episode.agentId} or tell the operator.`
              : `Agent blocked: ${episode.agentId} has been waiting at a dialog or permission prompt for a long time. Its pane needs an answer from the operator; messages to it wait until then. ${
                  this.areas.promptRelay.enabled
                    ? `Run cstan prompt show ${episode.agentId}.`
                    : `Look with cstan observe ${episode.agentId}.`
                }`;
          if (
            this.#queuePmNotice(
              episode.kind,
              episode.agentId,
              String(episode.episodeMs),
              body,
              now,
            )
          )
            queued += 1;
        }
        return {
          value: { queued },
          event: {
            entityType: "pm_notice",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: { queued },
          },
        };
      },
    );
  }

  /** Records a wake line before it is typed, so a crash after the record never types it twice. */
  recordPmWake(
    context: MutationContext,
    messageId: string,
  ): { readonly wakes: number } {
    safeId(messageId, "message id");
    return this.kernel.mutate(
      context,
      "pm.wake",
      "controller:reconcile",
      { messageId },
      () => {
        const row = this.areas.messages.messageRow(messageId);
        const recipient =
          row === undefined
            ? undefined
            : this.kernel.agentRow(row.recipient_agent_id);
        if (row?.state !== "queued" || recipient?.kind !== "PM")
          throw new ControllerError(
            "the message is no longer waiting for the PM",
          );
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            "INSERT INTO pm_wakes(project_id, wake_id, message_id, sent_at) VALUES (?, ?, ?, ?)",
          )
          .run(this.kernel.projectId, randomUUID(), messageId, now);
        const wakes = (
          this.kernel.database
            .prepare(
              "SELECT COUNT(*) AS n FROM pm_wakes WHERE project_id = ? AND message_id = ?",
            )
            .get(this.kernel.projectId, messageId) as { n: number }
        ).n;
        return {
          value: { wakes },
          event: {
            entityType: "message_note",
            entityId: messageId,
            stateVersion: 0,
            details: { wake: wakes },
          },
        };
      },
    );
  }

  #supervisionCheckDue(intervalSeconds: number): boolean {
    const active = this.kernel.database
      .prepare(
        "SELECT agent_id, kind FROM agents WHERE project_id = ? AND state = 'active' AND kind IN ('Supervisor', 'Developer', 'Verifier')",
      )
      .all(this.kernel.projectId) as Array<{ agent_id: string; kind: string }>;
    const supervisors = active.filter((a) => a.kind === "Supervisor");
    if (
      supervisors.length !== 1 ||
      active.length === 1 ||
      this.noticeParties() === undefined
    )
      return false;
    const last = this.kernel.database
      .prepare(
        `SELECT MAX(c.queued_at) AS at FROM supervision_checks c JOIN messages m ON m.project_id = c.project_id AND m.message_id = c.message_id
         WHERE c.project_id = ? AND m.recipient_agent_id = ?`,
      )
      .get(this.kernel.projectId, supervisors[0]!.agent_id) as {
      at: string | null;
    };
    return (
      last.at === null ||
      this.kernel.clock().getTime() - Date.parse(last.at) >=
        intervalSeconds * 1000
    );
  }

  /**
   * Queues the routine check for the one active Supervisor when a worker is
   * active and the last check is at least `intervalSeconds` old. A check that
   * is still open when the next is due is cancelled and replaced, so a
   * Supervisor's queue is never blocked by its own checks.
   */
  queueSupervisionCheck(
    context: MutationContext,
    intervalSeconds: number,
  ): { readonly queued: boolean; readonly cancelled: number } {
    this.kernel.authorize(context.credential, "controller:reconcile");
    if (!Number.isSafeInteger(intervalSeconds) || intervalSeconds < 1)
      throw new TypeError("the check interval must be a positive integer");
    // Nothing is written, not even an event, while no check is due: the tick asks every few seconds.
    if (!this.#supervisionCheckDue(intervalSeconds))
      return { queued: false, cancelled: 0 };
    return this.kernel.mutate(
      context,
      "supervision.check",
      "controller:reconcile",
      { intervalSeconds },
      (actor) => {
        const none = {
          value: { queued: false, cancelled: 0 },
          event: {
            entityType: "supervision",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: { queued: false },
          },
        };
        const active = this.kernel.database
          .prepare(
            "SELECT * FROM agents WHERE project_id = ? AND state = 'active' AND kind IN ('Supervisor', 'Developer', 'Verifier')",
          )
          .all(this.kernel.projectId) as AgentRow[];
        const supervisors = active.filter((a) => a.kind === "Supervisor");
        const workers = active.filter((a) => a.kind !== "Supervisor");
        const parties = this.noticeParties();
        if (supervisors.length !== 1 || workers.length === 0 || !parties)
          return none;
        const supervisor = supervisors[0]!;
        const checks = this.kernel.database
          .prepare(
            `SELECT m.* FROM messages m JOIN supervision_checks c ON c.project_id = m.project_id AND c.message_id = m.message_id
             WHERE m.project_id = ? AND m.recipient_agent_id = ? ORDER BY m.sequence DESC`,
          )
          .all(this.kernel.projectId, supervisor.agent_id) as MessageRow[];
        const nowMs = this.kernel.clock().getTime();
        const last = checks[0];
        if (
          last !== undefined &&
          nowMs - Date.parse(last.queued_at) < intervalSeconds * 1000
        )
          return none;
        const now = this.kernel.now();
        let cancelled = 0;
        for (const open of checks) {
          if (isFinalState(open.state)) continue;
          const version = this.areas.messages.updateMessage(
            open,
            "cancelled",
            { state_reason: "superseded_check" },
            now,
          );
          this.areas.messages.appendMessageEvent(actor, context, {
            messageId: open.message_id,
            from: open.state,
            to: "cancelled",
            stateVersion: version,
            details: { reason: "superseded_check" },
          });
          cancelled += 1;
        }
        const body = SUPERVISION_CHECK_TEXT;
        const messageId = this.areas.messages.insertQueuedMessage(
          parties.controllerActorId,
          supervisor,
          body,
          sha256(body),
          now,
        );
        this.kernel.database
          .prepare(
            "INSERT INTO supervision_checks(project_id, message_id, queued_at) VALUES (?, ?, ?)",
          )
          .run(this.kernel.projectId, messageId, now);
        return {
          value: { queued: true, cancelled },
          event: {
            entityType: "supervision",
            entityId: this.kernel.projectId,
            stateVersion: 0,
            details: { queued: true, cancelled },
          },
        };
      },
    );
  }

  #evaluateMessaging(timers: MessagingTimers): MessagingEvaluation {
    const nowMs = this.kernel.clock().getTime();
    const wakes = new Map<string, { count: number; lastMs: number }>(
      (
        this.kernel.database
          .prepare(
            "SELECT message_id, COUNT(*) AS n, MAX(sent_at) AS last FROM pm_wakes WHERE project_id = ? GROUP BY message_id",
          )
          .all(this.kernel.projectId) as Array<{
          message_id: string;
          n: number;
          last: string;
        }>
      ).map((row) => [
        row.message_id,
        { count: row.n, lastMs: Date.parse(row.last) },
      ]),
    );
    const messages: MessageFacts[] = (
      this.kernel.database
        .prepare(
          `SELECT m.*, EXISTS (SELECT 1 FROM message_input_clears c WHERE c.project_id = m.project_id AND c.message_id = m.message_id AND c.deferral_count = m.deferral_count) AS input_clear_recorded
           FROM messages m JOIN agents a ON a.project_id = m.project_id AND a.agent_id = m.recipient_agent_id
           WHERE m.project_id = ? AND a.state = 'active' AND m.state NOT IN ('acked', 'acked_late', 'cancelled')
           ORDER BY m.sequence`,
        )
        .all(this.kernel.projectId) as Array<
        MessageRow & { input_clear_recorded: number }
      >
    ).map((row) => ({
      wakeCount: wakes.get(row.message_id)?.count ?? 0,
      lastWakeMs: wakes.get(row.message_id)?.lastMs ?? null,
      messageId: row.message_id,
      recipientAgentId: row.recipient_agent_id,
      state: row.state,
      sequence: row.sequence,
      queuedMs: Math.max(
        Date.parse(row.queued_at),
        this.areas.pauses.resumedMsFor(row.recipient_agent_id),
      ),
      sentMs: row.sent_at === null ? null : Date.parse(row.sent_at),
      deferredMs:
        row.deferred_at === null
          ? null
          : Math.max(
              Date.parse(row.deferred_at),
              this.areas.pauses.resumedMsFor(row.recipient_agent_id),
            ),
      deferredReason: row.deferred_reason,
      inputClearRecorded: row.input_clear_recorded === 1,
      lastNotifiedMs:
        row.last_notified_at === null ? null : Date.parse(row.last_notified_at),
    }));
    const agentRows = this.kernel.database
      .prepare(
        "SELECT agent_id, kind, last_activity_at FROM agents WHERE project_id = ? AND state = 'active'",
      )
      .all(this.kernel.projectId) as Array<{
      agent_id: string;
      kind: AgentFacts["kind"];
      last_activity_at: string;
    }>;
    const pauseState = this.areas.pauses.pauseState();
    const agents: AgentFacts[] = agentRows.map((row) => {
      // A pause covers the time up to its resume: timers start again from there, so a held message does not expire or stall the moment it is released.
      const resumedMs = this.areas.pauses.resumedMsFor(row.agent_id);
      const lastActivityMs = Math.max(
        Date.parse(row.last_activity_at),
        resumedMs,
      );
      // No timer looks further back than the agent's last activity or its
      // oldest open message, so older history cannot change any result.
      const cutoffMs = Math.min(
        lastActivityMs,
        ...messages
          .filter((message) => message.recipientAgentId === row.agent_id)
          .map((message) => message.queuedMs),
      );
      const cutoff = new Date(cutoffMs).toISOString();
      const entries = this.kernel.database
        .prepare(
          `SELECT herdr_state, observed_at FROM agent_state_history
           WHERE project_id = ? AND agent_id = ? AND observed_at >= ? ORDER BY sequence`,
        )
        .all(this.kernel.projectId, row.agent_id, cutoff) as Array<{
        herdr_state: HerdrState;
        observed_at: string;
      }>;
      const before = this.kernel.database
        .prepare(
          `SELECT herdr_state, observed_at FROM agent_state_history
           WHERE project_id = ? AND agent_id = ? AND observed_at < ? ORDER BY sequence DESC LIMIT 1`,
        )
        .get(this.kernel.projectId, row.agent_id, cutoff) as
        { herdr_state: HerdrState; observed_at: string } | undefined;
      const waits = this.kernel.database
        .prepare(
          `SELECT started_at, ended_at FROM agent_waits
           WHERE project_id = ? AND agent_id = ? AND (ended_at IS NULL OR ended_at >= ?)`,
        )
        .all(this.kernel.projectId, row.agent_id, cutoff) as Array<{
        started_at: string;
        ended_at: string | null;
      }>;
      return {
        agentId: row.agent_id,
        kind: row.kind,
        paused:
          pauseState.agents.some((pause) => pause.agentId === row.agent_id) ||
          (pauseState.run !== null && row.kind !== "PM"),
        lastActivityMs,
        observations: (before === undefined
          ? entries
          : [before, ...entries]
        ).map((entry) => ({
          state: entry.herdr_state,
          atMs: Date.parse(entry.observed_at),
        })),
        waits: waits.map((entry) => ({
          startMs: Date.parse(entry.started_at),
          endMs: entry.ended_at === null ? null : Date.parse(entry.ended_at),
        })),
      };
    });
    return evaluateMessaging(agents, messages, nowMs, timers);
  }

  closeWaits(agentId: string, now: string): void {
    this.kernel.database
      .prepare(
        "UPDATE agent_waits SET ended_at = MAX(?, started_at) WHERE project_id = ? AND agent_id = ? AND ended_at IS NULL",
      )
      .run(now, this.kernel.projectId, agentId);
  }

  #waitAlreadyClosedFor(actor: AuthenticatedActor, waitId: string): boolean {
    const wait = this.kernel.database
      .prepare(
        "SELECT agent_id, ended_at FROM agent_waits WHERE project_id = ? AND wait_id = ?",
      )
      .get(this.kernel.projectId, waitId) as
      { agent_id: string; ended_at: string | null } | undefined;
    if (wait === undefined || wait.ended_at === null) return false;
    return (
      actor.capabilities.has("controller:reconcile") ||
      this.kernel.agentByActor(actor.actorId)?.agent_id === wait.agent_id
    );
  }

  #endWait(
    actor: AuthenticatedActor,
    action: string,
    waitId: string,
  ): MutationOutput<{ readonly ended: boolean } | MessageRejection> {
    const wait = this.kernel.database
      .prepare(
        "SELECT agent_id, ended_at FROM agent_waits WHERE project_id = ? AND wait_id = ?",
      )
      .get(this.kernel.projectId, waitId) as
      { agent_id: string; ended_at: string | null } | undefined;
    const caller = this.kernel.agentByActor(actor.actorId);
    const allowed =
      actor.capabilities.has("controller:reconcile") ||
      (caller !== undefined && caller.agent_id === wait?.agent_id);
    if (wait === undefined || !allowed)
      return this.areas.messages.reject(actor, action, {
        code: wait === undefined ? "unknown_wait" : "not_wait_owner",
        message: "the wait does not exist or belongs to another agent",
      });
    const now = this.kernel.now();
    if (wait.ended_at === null)
      this.kernel.database
        .prepare(
          "UPDATE agent_waits SET ended_at = MAX(?, started_at) WHERE project_id = ? AND wait_id = ?",
        )
        .run(now, this.kernel.projectId, waitId);
    if (caller !== undefined && caller.agent_id === wait.agent_id)
      this.kernel.touchAgent(caller.agent_id, now);
    return {
      value: { ended: wait.ended_at === null },
      event: {
        entityType: "agent_wait",
        entityId: waitId,
        stateVersion: 1,
        fromState: "open",
        toState: "closed",
      },
    };
  }

  /** A controller message to one active agent; null when the agent has ended. The caller owns the transaction. */
  noticeToAgent(agentId: string, body: string, now: string): string | null {
    const agent = this.kernel.agentRow(agentId);
    if (agent?.state !== "active") return null;
    return this.areas.messages.insertQueuedMessage(
      this.kernel.internalActorId,
      agent,
      body,
      sha256(body),
      now,
    );
  }
}

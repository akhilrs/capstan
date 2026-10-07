import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { randomUUID } from "node:crypto";
import {
  AuthorizationError,
  authenticateActor,
  requireCapability,
  type AuthenticatedActor,
} from "./auth.js";
import { sha256 } from "./canonical.js";
import {
  DEFERRAL_REASONS,
  FINAL_STATES,
  MESSAGE_STATES,
  RESOLUTION_DECISIONS,
  isFinalState,
  isLegalTransition,
  queueHead,
  resolutionTarget,
  type DeferralReason,
  type MessageState,
  type ResolutionDecision,
} from "./messaging.js";
import {
  type Capability,
  type MessageInput,
  type MessageRecord,
  type MessageRejectionRecord,
  type MutationContext,
} from "./types.js";
import { ControllerError, MessageTransitionError } from "./errors.js";
import {
  MAX_MESSAGE_BYTES,
  MAX_INPUT_CLEAR_BYTES,
  type MessageRejection,
  type AgentRow,
  type MessageRow,
  type MutationOutput,
} from "./records.js";
import {
  safeId,
  safeText,
  cancelledReason,
  isMessageRejection,
  messageRecord,
} from "./helpers.js";

/**
 * The states that are not final, as an SQL list. A query on it reads `messages` through `INDEXED BY messages_by_state`
 * (migration 0036): without table statistics SQLite scans the whole table, acked history and message bodies included.
 */
export const OPEN_MESSAGE_STATES_SQL = MESSAGE_STATES.filter(
  (state) => !FINAL_STATES.includes(state),
)
  .map((state) => `'${state}'`)
  .join(", ");

export class MessagesArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  enqueueMessage(
    context: MutationContext,
    input: MessageInput,
  ): { readonly messageId: string } {
    safeId(input.recipientAgentId, "recipient agent id");
    safeText(input.body, "message body", MAX_MESSAGE_BYTES, true);
    const bytes = Buffer.byteLength(input.body, "utf8");
    if (bytes > MAX_MESSAGE_BYTES)
      throw new TypeError(
        `message body must be at most ${MAX_MESSAGE_BYTES} bytes`,
      );
    const bodyHash = sha256(input.body);
    return this.messageMutation(
      context,
      "message.enqueue",
      "message:send",
      { recipientAgentId: input.recipientAgentId, bodyHash, bodyBytes: bytes },
      (actor) => {
        const agent = this.kernel.agentRow(input.recipientAgentId);
        if (agent?.state !== "active")
          return this.reject(actor, "message.enqueue", {
            code: "unknown_recipient",
            message: "message recipient is not an active agent",
          });
        const now = this.kernel.now();
        const messageId = this.insertQueuedMessage(
          actor.actorId,
          agent,
          input.body,
          bodyHash,
          now,
          input.actionNeeded === true,
        );
        const senderAgent = this.kernel.agentByActor(actor.actorId);
        if (senderAgent !== undefined)
          this.kernel.touchAgent(senderAgent.agent_id, now);
        return {
          value: { messageId },
          event: {
            entityType: "message",
            entityId: messageId,
            stateVersion: 0,
            toState: "queued",
            details: { recipientAgentId: agent.agent_id, bodyHash },
          },
        };
      },
    );
  }

  /** Queues a message row for an active recipient and returns its id. The caller owns the transaction. */
  insertQueuedMessage(
    senderActorId: string,
    recipient: AgentRow,
    body: string,
    bodyHash: string,
    now: string,
    actionNeeded: boolean,
  ): string {
    const messageId = randomUUID();
    const sequence = (
      this.kernel.database
        .prepare(
          "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM messages WHERE project_id = ?",
        )
        .get(this.kernel.projectId) as { next: number }
    ).next;
    this.kernel.database
      .prepare(
        `INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id,
          body, body_hash, state, state_version, queued_at, deferral_count, send_attempts, created_at, updated_at, action_needed)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', 0, ?, 0, 0, ?, ?, ?)`,
      )
      .run(
        this.kernel.projectId,
        messageId,
        sequence,
        recipient.agent_id,
        recipient.generation,
        senderActorId,
        body,
        bodyHash,
        now,
        now,
        now,
        actionNeeded ? 1 : 0,
      );
    return messageId;
  }

  message(messageId: string): MessageRecord | undefined {
    this.kernel.assertOpen();
    safeId(messageId, "message id");
    const row = this.messageRow(messageId);
    return row === undefined ? undefined : messageRecord(row);
  }

  messagesFor(agentId: string): readonly MessageRecord[] {
    this.kernel.assertOpen();
    safeId(agentId, "agent id");
    return this.messageRowsFor(agentId).map(messageRecord);
  }

  /** The agent's messages that are not final, oldest first; what `queueHead` and the PM mail summary read. */
  openMessagesFor(agentId: string): readonly MessageRecord[] {
    this.kernel.assertOpen();
    safeId(agentId, "agent id");
    return this.openMessageRowsFor(agentId).map(messageRecord);
  }

  /** Unresolved messages of every agent: notified ones first (the operator's bell depends on them), then oldest first by sequence. */
  unresolvedMessages(
    credential: string,
    limit: number,
  ): {
    readonly messages: readonly MessageRecord[];
    readonly truncated: boolean;
  } {
    this.kernel.authorize(credential, "controller:reconcile");
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 1000)
      throw new TypeError("limit must be an integer from 1 to 1000");
    const rows = this.kernel.database
      .prepare(
        `SELECT * FROM messages INDEXED BY messages_by_state WHERE project_id = ? AND state IN (${OPEN_MESSAGE_STATES_SQL}) ORDER BY (last_notified_at IS NULL), sequence LIMIT ?`,
      )
      .all(this.kernel.projectId, limit + 1) as MessageRow[];
    return {
      messages: rows.slice(0, limit).map(messageRecord),
      truncated: rows.length > limit,
    };
  }

  /** The newest input clears, without their text. */
  inputClears(
    credential: string,
    limit: number,
  ): readonly {
    readonly clearId: string;
    readonly messageId: string;
    readonly recordedAt: string;
  }[] {
    this.kernel.authorize(credential, "controller:reconcile");
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 1000)
      throw new TypeError("limit must be an integer from 1 to 1000");
    return (
      this.kernel.database
        .prepare(
          "SELECT clear_id, message_id, created_at FROM message_input_clears WHERE project_id = ? ORDER BY created_at DESC, clear_id LIMIT ?",
        )
        .all(this.kernel.projectId, limit) as Array<{
        clear_id: string;
        message_id: string;
        created_at: string;
      }>
    ).map((row) => ({
      clearId: row.clear_id,
      messageId: row.message_id,
      recordedAt: row.created_at,
    }));
  }

  /** Who an actor is, for showing a message's sender; an actor of an earlier generation has no agent id. */
  senderOf(actorId: string): {
    readonly role: string;
    readonly agentId: string | null;
  } {
    this.kernel.assertOpen();
    const row = this.kernel.database
      .prepare(
        `SELECT a.role AS kind, g.agent_id AS agent_id, g.role_name AS role_name
         FROM actors a LEFT JOIN agents g ON g.project_id = a.project_id AND g.actor_id = a.actor_id
         WHERE a.project_id = ? AND a.actor_id = ?`,
      )
      .get(this.kernel.projectId, actorId) as
      | { kind: string; agent_id: string | null; role_name: string | null }
      | undefined;
    if (row === undefined) return { role: "unknown", agentId: null };
    if (row.kind === "operator") return { role: "operator", agentId: null };
    return { role: row.role_name ?? row.kind, agentId: row.agent_id };
  }

  messageRejections(): readonly MessageRejectionRecord[] {
    this.kernel.assertOpen();
    return (
      this.kernel.database
        .prepare(
          "SELECT rejection_id, message_id, action, code, from_state, attempted_state, actor_id, reason FROM message_rejections WHERE project_id = ? ORDER BY sequence",
        )
        .all(this.kernel.projectId) as Array<{
        rejection_id: string;
        message_id: string | null;
        action: string;
        code: string;
        from_state: string | null;
        attempted_state: string | null;
        actor_id: string;
        reason: string;
      }>
    ).map((row) => ({
      rejectionId: row.rejection_id,
      messageId: row.message_id,
      action: row.action,
      code: row.code,
      fromState: row.from_state,
      attemptedState: row.attempted_state,
      actorId: row.actor_id,
      reason: row.reason,
    }));
  }

  agentInbox(credential: string, agentId?: string): readonly MessageRecord[] {
    this.kernel.assertOpen();
    if (agentId !== undefined) safeId(agentId, "agent id");
    const actor = authenticateActor(
      this.kernel.database,
      this.kernel.projectId,
      credential,
    );
    let target: string | undefined;
    if (actor.role === "operator") target = agentId;
    else {
      requireCapability(actor, "message:receive");
      target = this.kernel.agentByActor(actor.actorId)?.agent_id;
      if (agentId !== undefined && agentId !== target)
        throw new AuthorizationError("an agent reads only its own inbox");
    }
    if (target === undefined) return [];
    const rows = this.messageRowsFor(target);
    const head = queueHead(rows);
    return rows
      .filter(
        (row) =>
          row.message_id === head?.message_id ||
          row.state === "sent" ||
          row.state === "unacked",
      )
      .map(messageRecord);
  }

  pullMessage(context: MutationContext): {
    readonly message: MessageRecord | null;
  } {
    const actor = this.kernel.authorize(context.credential, "message:receive");
    const agent = this.kernel.agentByActor(actor.actorId);
    if (agent?.kind === "PM" && !this.kernel.hasStoredRequest(context)) {
      const head = queueHead(this.messageRowsFor(agent.agent_id));
      if (head?.state !== "queued") return { message: null };
      if (this.areas.pauses.isDeliveryPaused(agent.agent_id))
        return { message: null };
    }
    return this.messageMutation(
      context,
      "message.pull",
      "message:receive",
      {},
      (caller) => {
        const recipient = this.kernel.agentByActor(caller.actorId);
        if (recipient === undefined)
          return this.reject(caller, "message.pull", {
            code: "not_an_agent",
            message: "the caller is not the current actor of an active agent",
          });
        if (recipient.kind !== "PM")
          return this.reject(caller, "message.pull", {
            code: "pull_not_allowed",
            message: "only the PM pulls messages; workers receive pushes",
          });
        const head = queueHead(this.messageRowsFor(recipient.agent_id));
        if (
          head?.state !== "queued" ||
          this.areas.pauses.isDeliveryPaused(recipient.agent_id)
        )
          return this.reject(caller, "message.pull", {
            code: "nothing_to_pull",
            message: "the queue head is not waiting to be sent",
          });
        const now = this.kernel.now();
        const version = this.updateMessage(
          head,
          "sent",
          {
            sent_at: now,
            send_attempts: head.send_attempts + 1,
          },
          now,
        );
        this.kernel.touchAgent(recipient.agent_id, now);
        return {
          value: { message: messageRecord(this.messageRow(head.message_id)!) },
          event: {
            entityType: "message",
            entityId: head.message_id,
            stateVersion: version,
            fromState: "queued",
            toState: "sent",
          },
        };
      },
    );
  }

  /**
   * An agent reads its mail (the PM too): every queued or deferred message
   * becomes sent, in sequence order, one ledger event each. It stops at the
   * first expired or failed message, which is the PM's to resolve, and pulls
   * nothing while the
   * agent's delivery is paused. A message already sent or unacked is kept.
   */
  pullPending(credential: string): readonly MessageRecord[] {
    this.kernel.authorize(credential, "message:receive");
    const actor = authenticateActor(
      this.kernel.database,
      this.kernel.projectId,
      credential,
    );
    const agent = this.kernel.agentByActor(actor.actorId);
    if (agent === undefined) return [];
    if (this.areas.pauses.isDeliveryPaused(agent.agent_id)) return [];
    const pulled: MessageRecord[] = [];
    for (const row of this.messageRowsFor(agent.agent_id)) {
      if (isFinalState(row.state) || row.state === "sent") continue;
      if (row.state === "unacked") continue;
      if (row.state !== "queued" && row.state !== "deferred") break;
      const id = randomUUID();
      const record = this.messageMutation<MessageRecord>(
        {
          credential,
          requestId: `req-${id}`,
          idempotencyKey: `idem-${id}`,
          expectedVersion: this.kernel.stateVersion,
          inputRevision: this.kernel.inputRevision,
        },
        "message.pull",
        "message:receive",
        { messageId: row.message_id },
        (caller) => {
          const current = this.messageRow(row.message_id);
          if (current?.state !== "queued" && current?.state !== "deferred")
            return this.reject(caller, "message.pull", {
              messageId: row.message_id,
              code: "nothing_to_pull",
              message: "the message is no longer waiting to be sent",
              fromState: current?.state,
            });
          const now = this.kernel.now();
          const version = this.updateMessage(
            current,
            "sent",
            {
              sent_at: now,
              send_attempts: current.send_attempts + 1,
            },
            now,
          );
          if (pulled.length === 0) this.kernel.touchAgent(agent.agent_id, now);
          return {
            value: messageRecord(this.messageRow(row.message_id)!),
            event: {
              entityType: "message",
              entityId: row.message_id,
              stateVersion: version,
              fromState: current.state,
              toState: "sent",
            },
          };
        },
      );
      pulled.push(record);
    }
    return pulled;
  }

  /** What waits for the caller: its queued, deferred, sent and unacked messages. Read-only; nothing is written. */
  unreadSummary(credential: string): {
    readonly count: number;
    readonly oldestQueuedAt: string | null;
    readonly messageIds: readonly string[];
    readonly actionNeeded: number;
  } {
    this.kernel.assertOpen();
    const actor = authenticateActor(
      this.kernel.database,
      this.kernel.projectId,
      credential,
    );
    const agent =
      actor.role === "operator"
        ? undefined
        : this.kernel.agentByActor(actor.actorId);
    const waiting =
      agent === undefined
        ? []
        : this.messageRowsFor(agent.agent_id).filter(
            (row) =>
              row.state === "queued" ||
              row.state === "deferred" ||
              row.state === "sent" ||
              row.state === "unacked",
          );
    let oldest: string | null = null;
    for (const row of waiting)
      if (oldest === null || row.queued_at < oldest) oldest = row.queued_at;
    return {
      count: waiting.length,
      oldestQueuedAt: oldest,
      messageIds: waiting.map((row) => row.message_id),
      actionNeeded: waiting.filter((row) => row.action_needed === 1).length,
    };
  }

  recordDeferral(
    context: MutationContext,
    messageId: string,
    reason: DeferralReason,
  ): MessageRecord {
    safeId(messageId, "message id");
    if (!DEFERRAL_REASONS.includes(reason))
      throw new TypeError("unknown deferral reason");
    this.kernel.authorize(context.credential, "controller:reconcile");
    const current = this.messageRow(messageId);
    if (
      current?.state === "deferred" &&
      current.deferred_reason === reason &&
      !this.kernel.hasStoredRequest(context)
    )
      return messageRecord(current);
    return this.messageMutation(
      context,
      "message.defer",
      "controller:reconcile",
      { messageId, reason },
      (actor) => {
        const row = this.messageRow(messageId);
        const refused = this.#refuseDelivery(
          actor,
          "message.defer",
          row,
          "deferred",
        );
        if (refused) return refused;
        const now = this.kernel.now();
        const from = row!.state;
        const version =
          from === "queued"
            ? this.updateMessage(
                row!,
                "deferred",
                {
                  deferred_at: now,
                  deferred_reason: reason,
                  deferral_count: row!.deferral_count + 1,
                },
                now,
              )
            : this.updateMessage(
                row!,
                "deferred",
                {
                  // The clock restarts with the reason, so the maximum deferral counts from when the input line was first seen non-empty, not from when the worker first looked busy.
                  ...(row!.deferred_reason !== reason
                    ? { deferred_at: now }
                    : {}),
                  deferred_reason: reason,
                  deferral_count:
                    reason === "input_not_empty"
                      ? row!.deferral_count + 1
                      : row!.deferral_count,
                },
                now,
              );
        return {
          value: messageRecord(this.messageRow(messageId)!),
          event: {
            entityType: from === "queued" ? "message" : "message_note",
            entityId: messageId,
            stateVersion: version,
            fromState: from,
            toState: "deferred",
            details: { reason },
          },
        };
      },
    );
  }

  recordInputClear(
    context: MutationContext,
    messageId: string,
    text: string,
  ): { readonly clearId: string } {
    safeId(messageId, "message id");
    if (
      typeof text !== "string" ||
      text.length === 0 ||
      !text.isWellFormed() ||
      Buffer.byteLength(text, "utf8") > MAX_INPUT_CLEAR_BYTES
    )
      throw new TypeError(
        `input text must be non-empty, well-formed and at most ${MAX_INPUT_CLEAR_BYTES} bytes`,
      );
    const textHash = sha256(text);
    return this.messageMutation(
      context,
      "message.input_clear",
      "controller:reconcile",
      { messageId, textHash },
      (actor) => {
        const row = this.messageRow(messageId);
        if (
          row?.state !== "deferred" ||
          row.deferred_reason !== "input_not_empty"
        )
          return this.reject(actor, "message.input_clear", {
            messageId,
            code: "not_input_deferred",
            message:
              "input text is recorded only for a message deferred because the input line is not empty",
            fromState: row?.state,
          });
        const clearId = randomUUID();
        this.kernel.database
          .prepare(
            "INSERT INTO message_input_clears(project_id, clear_id, message_id, deferral_count, text, text_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
          )
          .run(
            this.kernel.projectId,
            clearId,
            messageId,
            row.deferral_count,
            text,
            textHash,
            this.kernel.now(),
          );
        return {
          value: { clearId },
          event: {
            entityType: "message_note",
            entityId: messageId,
            stateVersion: row.state_version,
            fromState: "deferred",
            toState: "deferred",
            details: { inputCleared: true, textHash },
          },
        };
      },
    );
  }

  recordSent(context: MutationContext, messageId: string): MessageRecord {
    safeId(messageId, "message id");
    return this.messageMutation(
      context,
      "message.sent",
      "controller:reconcile",
      { messageId },
      (actor) => {
        const row = this.messageRow(messageId);
        const refused = this.#refuseDelivery(
          actor,
          "message.sent",
          row,
          "sent",
        );
        if (refused) return refused;
        const now = this.kernel.now();
        const from = row!.state;
        const version = this.updateMessage(
          row!,
          "sent",
          {
            sent_at: now,
            send_attempts: row!.send_attempts + 1,
          },
          now,
        );
        return {
          value: messageRecord(this.messageRow(messageId)!),
          event: {
            entityType: "message",
            entityId: messageId,
            stateVersion: version,
            fromState: from,
            toState: "sent",
          },
        };
      },
    );
  }

  recordFailure(
    context: MutationContext,
    messageId: string,
    reason: string,
  ): MessageRecord {
    safeId(messageId, "message id");
    safeText(reason, "failure reason", 500, false);
    return this.messageMutation(
      context,
      "message.fail",
      "controller:reconcile",
      { messageId, reason },
      (actor) => {
        const row = this.messageRow(messageId);
        const refused = this.#refuseDelivery(
          actor,
          "message.fail",
          row,
          "failed",
        );
        if (refused) return refused;
        const from = row!.state;
        const failedAt = this.kernel.now();
        const version = this.updateMessage(
          row!,
          "failed",
          { state_reason: reason },
          failedAt,
        );
        this.areas.messageNotices.queueDeliveryNotice(
          row!,
          "failed",
          failedAt,
          reason,
        );
        return {
          value: messageRecord(this.messageRow(messageId)!),
          event: {
            entityType: "message",
            entityId: messageId,
            stateVersion: version,
            fromState: from,
            toState: "failed",
            details: { reason },
          },
        };
      },
    );
  }

  ackMessage(context: MutationContext, messageId: string): MessageRecord {
    safeId(messageId, "message id");
    return this.messageMutation(
      context,
      "message.ack",
      "message:receive",
      { messageId },
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        const row = this.messageRow(messageId);
        if (agent === undefined)
          return this.reject(actor, "message.ack", {
            messageId,
            code: "not_an_agent",
            message: "the caller is not the current actor of an active agent",
            fromState: row?.state,
          });
        if (row === undefined)
          return this.reject(actor, "message.ack", {
            messageId,
            code: "unknown_message",
            message: "message does not exist",
          });
        if (row.recipient_agent_id !== agent.agent_id)
          return this.reject(actor, "message.ack", {
            messageId,
            code: "not_recipient",
            message: "only the recipient agent may ack a message",
            fromState: row?.state,
          });
        const to: MessageState =
          row.state === "unacked" ? "acked_late" : "acked";
        if (!isLegalTransition(row.state, to))
          return this.reject(actor, "message.ack", {
            messageId,
            code: "illegal_transition",
            message: `a message in state ${row.state} cannot be acked`,
            fromState: row.state,
            attemptedState: to,
          });
        const now = this.kernel.now();
        const version = this.updateMessage(row, to, { acked_at: now }, now);
        this.kernel.touchAgent(agent.agent_id, now);
        return {
          value: messageRecord(this.messageRow(messageId)!),
          event: {
            entityType: "message",
            entityId: messageId,
            stateVersion: version,
            fromState: row.state,
            toState: to,
          },
        };
      },
    );
  }

  resolveMessage(
    context: MutationContext,
    messageId: string,
    decision: ResolutionDecision,
    note?: string,
  ): MessageRecord {
    safeId(messageId, "message id");
    if (!RESOLUTION_DECISIONS.includes(decision))
      throw new TypeError("unknown resolution decision");
    if (note !== undefined) safeText(note, "resolution note", 1000, true);
    return this.messageMutation(
      context,
      "message.resolve",
      "message:resolve",
      { messageId, decision, note: note ?? null },
      (actor) => {
        const row = this.messageRow(messageId);
        if (row === undefined)
          return this.reject(actor, "message.resolve", {
            messageId,
            code: "unknown_message",
            message: "message does not exist",
          });
        const recipient = this.kernel.agentRow(row.recipient_agent_id);
        if (recipient?.kind === "PM" && actor.role !== "operator")
          return this.reject(actor, "message.resolve", {
            messageId,
            code: "operator_only",
            message:
              "a message addressed to the PM is resolved only by the operator",
            fromState: row.state,
          });
        if (decision === "retry" && recipient?.state !== "active")
          return this.reject(actor, "message.resolve", {
            messageId,
            code: "recipient_not_active",
            message:
              "the recipient agent has ended, so the message cannot be sent again",
            fromState: row.state,
          });
        const to = resolutionTarget(decision, row.state);
        if (to === undefined)
          return this.reject(actor, "message.resolve", {
            messageId,
            code: "illegal_resolution",
            message: `${decision} does not apply to a message in state ${row.state}`,
            fromState: row.state,
          });
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            "INSERT INTO message_resolutions(project_id, resolution_id, message_id, decision, decided_by, note, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
          )
          .run(
            this.kernel.projectId,
            randomUUID(),
            messageId,
            decision,
            actor.actorId,
            note ?? null,
            now,
          );
        const resolver = this.kernel.agentByActor(actor.actorId);
        if (resolver !== undefined)
          this.kernel.touchAgent(resolver.agent_id, now);
        const version =
          decision === "retry"
            ? this.updateMessage(
                row,
                "queued",
                {
                  queued_at: now,
                  deferred_at: null,
                  deferred_reason: null,
                  notified_at: null,
                  last_notified_at: null,
                  state_reason: null,
                },
                now,
              )
            : this.updateMessage(
                row,
                "cancelled",
                {
                  state_reason: cancelledReason(`resolution_${decision}`, row),
                },
                now,
              );
        return {
          value: messageRecord(this.messageRow(messageId)!),
          event: {
            entityType: "message",
            entityId: messageId,
            stateVersion: version,
            fromState: row.state,
            toState: to,
            details: { decision },
          },
        };
      },
    );
  }

  messageRow(messageId: string): MessageRow | undefined {
    return this.kernel.database
      .prepare("SELECT * FROM messages WHERE project_id = ? AND message_id = ?")
      .get(this.kernel.projectId, messageId) as MessageRow | undefined;
  }

  messageRowsFor(agentId: string): MessageRow[] {
    return this.kernel.database
      .prepare(
        "SELECT * FROM messages WHERE project_id = ? AND recipient_agent_id = ? ORDER BY sequence",
      )
      .all(this.kernel.projectId, agentId) as MessageRow[];
  }

  openMessageRowsFor(agentId: string): MessageRow[] {
    return this.kernel.database
      .prepare(
        `SELECT * FROM messages INDEXED BY messages_by_state WHERE project_id = ? AND recipient_agent_id = ? AND state IN (${OPEN_MESSAGE_STATES_SQL}) ORDER BY sequence`,
      )
      .all(this.kernel.projectId, agentId) as MessageRow[];
  }

  updateMessage(
    row: MessageRow,
    to: MessageState,
    extra: Readonly<Record<string, string | number | null>>,
    now: string,
  ): number {
    const columns = Object.keys(extra);
    const result = this.kernel.database
      .prepare(
        `UPDATE messages SET state = ?, state_version = state_version + 1, updated_at = ?${columns
          .map((column) => `, ${column} = ?`)
          .join("")} WHERE project_id = ? AND message_id = ? AND state = ?`,
      )
      .run(
        to,
        now,
        ...columns.map((column) => extra[column]!),
        this.kernel.projectId,
        row.message_id,
        row.state,
      );
    if (result.changes !== 1)
      throw new ControllerError("message state changed during the mutation");
    return row.state_version + 1;
  }

  reject(
    actor: AuthenticatedActor,
    action: string,
    rejection: {
      readonly messageId?: string | undefined;
      readonly code: string;
      readonly message: string;
      readonly fromState?: string | undefined;
      readonly attemptedState?: string | undefined;
    },
  ): MutationOutput<MessageRejection> {
    const rejectionId = randomUUID();
    this.kernel.database
      .prepare(
        "INSERT INTO message_rejections(project_id, rejection_id, sequence, message_id, action, code, from_state, attempted_state, actor_id, reason, created_at) VALUES (?, ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM message_rejections WHERE project_id = ?), ?, ?, ?, ?, ?, ?, ?, ?)",
      )
      .run(
        this.kernel.projectId,
        rejectionId,
        this.kernel.projectId,
        rejection.messageId ?? null,
        action,
        rejection.code,
        rejection.fromState ?? null,
        rejection.attemptedState ?? null,
        actor.actorId,
        rejection.message,
        this.kernel.now(),
      );
    return {
      value: {
        rejected: true,
        code: rejection.code,
        message: rejection.message,
      },
      event: {
        entityType: "message_rejection",
        entityId: rejectionId,
        stateVersion: 0,
        ...(rejection.fromState === undefined
          ? {}
          : { fromState: rejection.fromState }),
        details: {
          action,
          code: rejection.code,
          messageId: rejection.messageId ?? null,
          attemptedState: rejection.attemptedState ?? null,
        },
      },
    };
  }

  messageMutation<T>(
    context: MutationContext,
    action: string,
    capability: Capability,
    payload: unknown,
    apply: (actor: AuthenticatedActor) => MutationOutput<T | MessageRejection>,
  ): T {
    const result = this.kernel.mutate<T | MessageRejection>(
      context,
      action,
      capability,
      payload,
      apply,
    );
    if (isMessageRejection(result))
      throw new MessageTransitionError(result.code, result.message);
    return result;
  }

  #refuseDelivery(
    actor: AuthenticatedActor,
    action: string,
    row: MessageRow | undefined,
    attempted: MessageState,
  ): MutationOutput<MessageRejection> | undefined {
    if (row === undefined)
      return this.reject(actor, action, {
        code: "unknown_message",
        message: "message does not exist",
        attemptedState: attempted,
      });
    if (
      !isLegalTransition(row.state, attempted) &&
      !(row.state === "deferred" && attempted === "deferred")
    )
      return this.reject(actor, action, {
        messageId: row.message_id,
        code: "illegal_transition",
        message: `a message in state ${row.state} cannot become ${attempted}`,
        fromState: row.state,
        attemptedState: attempted,
      });
    const head = queueHead(this.messageRowsFor(row.recipient_agent_id));
    if (head?.message_id !== row.message_id)
      return this.reject(actor, action, {
        messageId: row.message_id,
        code: "not_head",
        message: "an earlier message to the same recipient is unresolved",
        fromState: row.state,
        attemptedState: attempted,
      });
    return undefined;
  }

  appendMessageEvent(
    actor: AuthenticatedActor,
    context: MutationContext,
    event: {
      readonly messageId: string;
      readonly from: string;
      readonly to: string;
      readonly stateVersion: number;
      readonly details?: unknown;
    },
  ): void {
    this.kernel.appendEvent(actor, context, {
      entityType: "message",
      entityId: event.messageId,
      from: event.from,
      to: event.to,
      action: "message.transition",
      stateVersion: event.stateVersion,
      details: event.details ?? null,
    });
  }

  /** Cancels what is not final. `keepFailed` leaves a failed message failed (an ended agent's failures stay visible). */
  cancelMessagesOf(
    actor: AuthenticatedActor,
    context: MutationContext,
    agentId: string,
    reason: string,
    now: string,
    keepFailed = false,
  ): string[] {
    const cancelled: string[] = [];
    for (const row of this.messageRowsFor(agentId)) {
      if (isFinalState(row.state)) continue;
      if (keepFailed && row.state === "failed") continue;
      const version = this.updateMessage(
        row,
        "cancelled",
        { state_reason: cancelledReason(reason, row) },
        now,
      );
      this.appendMessageEvent(actor, context, {
        messageId: row.message_id,
        from: row.state,
        to: "cancelled",
        stateVersion: version,
        details: { reason },
      });
      cancelled.push(row.message_id);
    }
    return cancelled;
  }
}

import type { ControllerKernel } from "./kernel.js";
import { ControllerError } from "./errors.js";
import { safeId, safeText } from "./helpers.js";
import type { AgentRow, MutationEvent, PromptRelayRow } from "./records.js";
import { PROMPT_RELAY_TEXT_MAX_CHARS } from "./records.js";
import type {
  MutationContext,
  PromptRelayRecord,
  PromptRelayState,
} from "./types.js";
import {
  RELAY_PROMPT_MAX_BYTES,
  relayTextProblem,
  type CapturedPrompt,
  type PromptAnswer,
  type RelayOption,
} from "../herdr/prompt-relay.js";

/** The prompt relay: what the PM was shown, the answer typed back, and the captures that expire. */
export class PromptRelayArea {
  readonly #k: ControllerKernel;

  constructor(kernel: ControllerKernel) {
    this.#k = kernel;
  }

  #config: { enabled: boolean; captureTtlSeconds: number } = {
    enabled: false,
    captureTtlSeconds: 600,
  };

  /** Set once by the daemon from `[prompt_relay]`; off, no notice or status view mentions the relay. */
  configure(config: {
    readonly enabled: boolean;
    readonly captureTtlSeconds: number;
  }): void {
    this.#config = { ...config };
  }

  get enabled(): boolean {
    return this.#config.enabled;
  }

  #promptRelayRow(relayId: string): PromptRelayRow | undefined {
    return this.#k.database
      .prepare(
        "SELECT * FROM prompt_relays WHERE project_id = ? AND relay_id = ?",
      )
      .get(this.#k.projectId, relayId) as PromptRelayRow | undefined;
  }

  #promptRelayRecord(row: PromptRelayRow): PromptRelayRecord {
    return {
      relayId: row.relay_id,
      sequence: row.sequence,
      agentId: row.agent_id,
      paneId: row.pane_id,
      hostKind: row.host_kind,
      promptText: row.prompt_text,
      options: JSON.parse(row.options_json) as RelayOption[],
      promptSha: row.prompt_sha,
      hash12: row.prompt_sha.slice(0, 12),
      capturedByActorId: row.captured_by_actor_id,
      capturedAt: row.captured_at,
      expiresAt: row.expires_at,
      state: row.state,
      answer:
        row.answer_kind === null
          ? null
          : {
              kind: row.answer_kind,
              option: row.answer_option,
              widensPermissions: row.answer_widens_permissions === 1,
              text: row.answer_text,
            },
      answeredByActorId: row.answered_by_actor_id,
      answeredAt: row.answered_at,
      outcomeReason: row.outcome_reason,
      keys:
        row.keys_json === null ? null : (JSON.parse(row.keys_json) as string[]),
    };
  }

  #promptRelayEvent(
    relayId: string,
    fromState: PromptRelayState | undefined,
    toState: PromptRelayState,
    details: Record<string, unknown>,
  ): MutationEvent {
    return {
      entityType: "prompt_relay",
      entityId: relayId,
      stateVersion: 0,
      ...(fromState === undefined ? {} : { fromState }),
      toState,
      details,
    };
  }

  #activePmOf(actorId: string): AgentRow {
    const agent = this.#k.agentByActor(actorId);
    if (agent?.kind !== "PM")
      throw new ControllerError(
        "forbidden: only an active PM agent relays prompts",
      );
    return agent;
  }

  /** The stored relay when the answer may be typed to it now, else a ControllerError naming the reason. */
  #answerableRelay(
    relayId: string,
    hash: unknown,
    answer: PromptAnswer,
  ): PromptRelayRow {
    safeId(relayId, "relay id");
    const row = this.#promptRelayRow(relayId);
    if (row === undefined)
      throw new ControllerError(
        `unknown_relay: relay ${relayId} does not exist`,
      );
    if (row.state === "typing")
      throw new ControllerError(
        `relay_in_progress: relay ${relayId} is being typed`,
      );
    if (row.state !== "captured")
      throw new ControllerError(
        `relay_not_open: relay ${relayId} is ${row.state}`,
      );
    if (Date.parse(row.expires_at) <= Date.parse(this.#k.now()))
      throw new ControllerError(
        `capture_expired: the capture of relay ${relayId} expired; run cstan prompt show again`,
      );
    if (
      typeof hash !== "string" ||
      !/^[0-9a-f]{12,64}$/.test(hash) ||
      !row.prompt_sha.startsWith(hash)
    )
      throw new ControllerError(
        `hash_mismatch: --hash must be the hash shown by cstan prompt show for this relay`,
      );
    if (answer.kind !== "esc") {
      const options = JSON.parse(row.options_json) as RelayOption[];
      const option = options.find((entry) => entry.number === answer.number);
      if (option === undefined)
        throw new ControllerError(
          `no_such_option: option ${answer.number} is not in the shown prompt`,
        );
      if (answer.kind === "text") {
        if (!option.acceptsText)
          throw new ControllerError(
            `no_text_option: option ${answer.number} does not accept text`,
          );
        const problem = relayTextProblem(answer.text);
        if (problem !== undefined)
          throw new ControllerError(`text_refused: ${problem}`);
        if (answer.text !== answer.text.trim())
          throw new ControllerError(
            "text_refused: the text has leading or trailing whitespace",
          );
        if (Array.from(answer.text).length > PROMPT_RELAY_TEXT_MAX_CHARS)
          throw new ControllerError(
            `text_refused: the text is longer than ${PROMPT_RELAY_TEXT_MAX_CHARS} characters`,
          );
      }
    }
    return row;
  }

  #answerWidens(row: PromptRelayRow, answer: PromptAnswer): boolean {
    if (answer.kind === "esc") return false;
    const options = JSON.parse(row.options_json) as RelayOption[];
    return (
      options.find((entry) => entry.number === answer.number)
        ?.widensPermissions === true
    );
  }

  /** Read-only: the stored relay when `answer` may be typed to it, else a ControllerError. Nothing is changed. */
  checkPromptAnswer(
    relayId: string,
    hash: unknown,
    answer: PromptAnswer,
  ): PromptRelayRecord {
    this.#k.assertOpen();
    return this.#promptRelayRecord(
      this.#answerableRelay(relayId, hash, answer),
    );
  }

  promptRelay(relayId: string): PromptRelayRecord | undefined {
    this.#k.assertOpen();
    const row = this.#promptRelayRow(relayId);
    return row === undefined ? undefined : this.#promptRelayRecord(row);
  }

  /** Relays newest first. */
  listPromptRelays(limit = 20): readonly PromptRelayRecord[] {
    this.#k.assertOpen();
    return (
      this.#k.database
        .prepare(
          "SELECT * FROM prompt_relays WHERE project_id = ? ORDER BY sequence DESC LIMIT ?",
        )
        .all(this.#k.projectId, limit) as PromptRelayRow[]
    ).map((row) => this.#promptRelayRecord(row));
  }

  /** The status view: open captures and the last answers. */
  promptRelayStatus(): {
    readonly enabled: boolean;
    readonly openCaptures: readonly {
      readonly relayId: string;
      readonly agentId: string;
      readonly hash12: string;
      readonly state: PromptRelayState;
      readonly expiresAt: string;
    }[];
    readonly lastAnswers: readonly {
      readonly relayId: string;
      readonly agentId: string;
      readonly hash12: string;
      readonly state: PromptRelayState;
      readonly answer: PromptRelayRecord["answer"];
      readonly widensPermissions: boolean;
      readonly actorId: string | null;
      readonly at: string | null;
    }[];
  } {
    this.#k.assertOpen();
    const open = (
      this.#k.database
        .prepare(
          "SELECT * FROM prompt_relays WHERE project_id = ? AND state IN ('captured', 'typing') ORDER BY sequence",
        )
        .all(this.#k.projectId) as PromptRelayRow[]
    )
      .filter(
        (row) =>
          row.state === "typing" ||
          Date.parse(row.expires_at) > Date.parse(this.#k.now()),
      )
      .map((row) => this.#promptRelayRecord(row));
    const answered = (
      this.#k.database
        .prepare(
          "SELECT * FROM prompt_relays WHERE project_id = ? AND state IN ('answered', 'refused', 'failed') ORDER BY answered_at DESC, sequence DESC LIMIT 5",
        )
        .all(this.#k.projectId) as PromptRelayRow[]
    ).map((row) => this.#promptRelayRecord(row));
    return {
      enabled: this.#config.enabled,
      openCaptures: open.map((record) => ({
        relayId: record.relayId,
        agentId: record.agentId,
        hash12: record.hash12,
        state: record.state,
        expiresAt: record.expiresAt,
      })),
      lastAnswers: answered.map((record) => ({
        relayId: record.relayId,
        agentId: record.agentId,
        hash12: record.hash12,
        state: record.state,
        answer: record.answer,
        widensPermissions: record.answer?.widensPermissions === true,
        actorId: record.answeredByActorId,
        at: record.answeredAt,
      })),
    };
  }

  /**
   * Records what the PM was shown. An older captured row of the same agent becomes expired (superseded) in
   * the same transaction; a row that is being typed refuses a new capture.
   */
  recordPromptCapture(
    context: MutationContext,
    input: { readonly prompt: CapturedPrompt },
  ): PromptRelayRecord {
    const prompt = input.prompt;
    if (!/^[0-9a-f]{64}$/.test(prompt.promptSha))
      throw new TypeError(
        "the prompt hash must be 64 lowercase hex characters",
      );
    if (Buffer.byteLength(prompt.text, "utf8") > RELAY_PROMPT_MAX_BYTES)
      throw new TypeError(
        `the prompt text must be at most ${RELAY_PROMPT_MAX_BYTES} bytes`,
      );
    safeId(prompt.agentId, "agent id");
    return this.#k.mutate<PromptRelayRecord>(
      context,
      "prompt_relay.capture",
      "prompt:relay",
      { agentId: prompt.agentId, promptSha: prompt.promptSha },
      (actor) => {
        this.#activePmOf(actor.actorId);
        if (!this.#config.enabled)
          throw new ControllerError(
            "not_configured: the prompt relay is not enabled",
          );
        const agent = this.#k.agentRow(prompt.agentId);
        if (agent?.state !== "active")
          throw new ControllerError(
            "agent_not_active: the agent is not active",
          );
        const now = this.#k.now();
        const events: MutationEvent[] = [];
        const open = this.#k.database
          .prepare(
            "SELECT * FROM prompt_relays WHERE project_id = ? AND agent_id = ? AND state IN ('captured', 'typing')",
          )
          .all(this.#k.projectId, prompt.agentId) as PromptRelayRow[];
        if (open.some((row) => row.state === "typing"))
          throw new ControllerError(
            `relay_in_progress: relay ${open.find((row) => row.state === "typing")!.relay_id} is being typed`,
          );
        for (const row of open) {
          this.#k.database
            .prepare(
              "UPDATE prompt_relays SET state = 'expired', outcome_reason = 'superseded' WHERE project_id = ? AND relay_id = ?",
            )
            .run(this.#k.projectId, row.relay_id);
          events.push(
            this.#promptRelayEvent(row.relay_id, "captured", "expired", {
              reason: "superseded",
            }),
          );
        }
        const sequence = (
          this.#k.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM prompt_relays WHERE project_id = ?",
            )
            .get(this.#k.projectId) as { next: number }
        ).next;
        const relayId = `relay-${sequence}`;
        const expiresAt = new Date(
          Date.parse(now) + this.#config.captureTtlSeconds * 1000,
        ).toISOString();
        this.#k.database
          .prepare(
            `INSERT INTO prompt_relays(project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json,
               prompt_sha, captured_by_actor_id, captured_at, expires_at, state)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'captured')`,
          )
          .run(
            this.#k.projectId,
            relayId,
            sequence,
            prompt.agentId,
            prompt.paneId,
            prompt.hostKind,
            prompt.text,
            JSON.stringify(
              prompt.options.map((option) => ({
                number: option.number,
                text: option.text,
                acceptsText: option.acceptsText,
                widensPermissions: option.widensPermissions,
              })),
            ),
            prompt.promptSha,
            actor.actorId,
            now,
            expiresAt,
          );
        const record = this.#promptRelayRecord(this.#promptRelayRow(relayId)!);
        return {
          value: record,
          event: this.#promptRelayEvent(relayId, undefined, "captured", {
            agentId: prompt.agentId,
            hash12: record.hash12,
          }),
          extraEvents: events,
        };
      },
    );
  }

  /** The target of the launcher's beforeType: captured -> typing, with the answer, in one transaction that re-checks state, expiry and hash. */
  beginPromptAnswer(
    context: MutationContext,
    input: {
      readonly relayId: string;
      readonly hash: string;
      readonly answer: PromptAnswer;
    },
  ): PromptRelayRecord {
    return this.#k.mutate<PromptRelayRecord>(
      context,
      "prompt_relay.begin",
      "prompt:relay",
      {
        relayId: input.relayId,
        hash: input.hash,
        answer: input.answer,
      },
      (actor) => {
        this.#activePmOf(actor.actorId);
        const row = this.#answerableRelay(
          input.relayId,
          input.hash,
          input.answer,
        );
        this.#recordAnswer(row, input.answer, actor.actorId, "typing", {});
        return {
          value: this.#promptRelayRecord(this.#promptRelayRow(row.relay_id)!),
          event: this.#promptRelayEvent(row.relay_id, "captured", "typing", {
            answerKind: input.answer.kind,
          }),
        };
      },
    );
  }

  #recordAnswer(
    row: PromptRelayRow,
    answer: PromptAnswer,
    actorId: string,
    state: "typing" | "refused",
    outcome: { readonly reason?: string; readonly keys?: readonly string[] },
  ): void {
    const now = this.#k.now();
    this.#k.database
      .prepare(
        `UPDATE prompt_relays SET state = ?, answer_kind = ?, answer_option = ?, answer_widens_permissions = ?,
           answer_text = ?, answered_by_actor_id = ?, answered_at = ?, outcome_reason = ?, keys_json = ?
         WHERE project_id = ? AND relay_id = ?`,
      )
      .run(
        state,
        answer.kind,
        answer.kind === "esc" ? null : answer.number,
        this.#answerWidens(row, answer) ? 1 : 0,
        answer.kind === "text" ? answer.text : null,
        actorId,
        state === "typing" ? null : now,
        outcome.reason ?? null,
        outcome.keys === undefined ? null : JSON.stringify(outcome.keys),
        this.#k.projectId,
        row.relay_id,
      );
  }

  /** A launcher refusal before any key was typed: captured -> refused, with the answer that was asked for. */
  refusePromptAnswer(
    context: MutationContext,
    input: {
      readonly relayId: string;
      readonly answer: PromptAnswer;
      readonly reason: string;
    },
  ): PromptRelayRecord {
    return this.#k.mutate<PromptRelayRecord>(
      context,
      "prompt_relay.refuse",
      "prompt:relay",
      { relayId: input.relayId, answer: input.answer, reason: input.reason },
      (actor) => {
        this.#activePmOf(actor.actorId);
        const row = this.#promptRelayRow(safeId(input.relayId, "relay id"));
        if (row?.state !== "captured")
          throw new ControllerError(
            `relay_not_open: relay ${input.relayId} is not captured`,
          );
        const reason = safeText(input.reason, "reason", 1024, false);
        this.#recordAnswer(row, input.answer, actor.actorId, "refused", {
          reason,
          keys: [],
        });
        return {
          value: this.#promptRelayRecord(this.#promptRelayRow(row.relay_id)!),
          event: this.#promptRelayEvent(row.relay_id, "captured", "refused", {
            reason,
          }),
        };
      },
    );
  }

  /**
   * Ends a row that is typing, as the controller's own bookkeeping so it still works when the PM was released
   * meanwhile: typed -> answered; refused with no key sent -> refused; any key sent -> failed.
   */
  finishPromptAnswer(
    context: MutationContext,
    input: {
      readonly relayId: string;
      readonly outcome:
        | { readonly typed: true; readonly keys: readonly string[] }
        | {
            readonly typed: false;
            readonly reason: string;
            readonly keys: readonly string[];
            /** Set when keys may have been sent that are not in `keys`. */
            readonly failed?: boolean;
          };
    },
  ): PromptRelayRecord {
    return this.#k.mutate<PromptRelayRecord>(
      context,
      "prompt_relay.finish",
      "controller:reconcile",
      { relayId: input.relayId, outcome: input.outcome },
      () => {
        const row = this.#promptRelayRow(safeId(input.relayId, "relay id"));
        if (row?.state !== "typing")
          throw new ControllerError(
            `relay_not_typing: relay ${input.relayId} is not being typed`,
          );
        const outcome = input.outcome;
        const state: PromptRelayState = outcome.typed
          ? "answered"
          : outcome.keys.length === 0 && outcome.failed !== true
            ? "refused"
            : "failed";
        const reason = outcome.typed
          ? null
          : safeText(outcome.reason, "reason", 1024, false);
        this.#endTyping(row, state, reason, outcome.keys);
        return {
          value: this.#promptRelayRecord(this.#promptRelayRow(row.relay_id)!),
          event: this.#promptRelayEvent(row.relay_id, "typing", state, {
            reason,
            keys: outcome.keys,
          }),
        };
      },
    );
  }

  #endTyping(
    row: PromptRelayRow,
    state: "answered" | "refused" | "failed",
    reason: string | null,
    keys: readonly string[],
  ): void {
    this.#k.database
      .prepare(
        "UPDATE prompt_relays SET state = ?, answered_at = ?, outcome_reason = ?, keys_json = ? WHERE project_id = ? AND relay_id = ?",
      )
      .run(
        state,
        this.#k.now(),
        reason,
        JSON.stringify(keys),
        this.#k.projectId,
        row.relay_id,
      );
  }

  /** Read-only: whether a capture has passed its time to live. */
  hasExpiredPromptCaptures(): boolean {
    this.#k.assertOpen();
    const now = this.#k.now();
    return (
      this.#k.database
        .prepare(
          "SELECT 1 AS due FROM prompt_relays WHERE project_id = ? AND state = 'captured' AND expires_at <= ? LIMIT 1",
        )
        .get(this.#k.projectId, now) !== undefined
    );
  }

  /** Captures that passed their time to live become expired. */
  expirePromptCaptures(context: MutationContext): readonly string[] {
    return this.#k.mutate<readonly string[]>(
      context,
      "prompt_relay.expire",
      "controller:reconcile",
      {},
      () => {
        const now = this.#k.now();
        const expired: string[] = [];
        for (const row of this.#k.database
          .prepare(
            "SELECT * FROM prompt_relays WHERE project_id = ? AND state = 'captured' ORDER BY sequence",
          )
          .all(this.#k.projectId) as PromptRelayRow[])
          if (Date.parse(row.expires_at) <= Date.parse(now)) {
            this.#k.database
              .prepare(
                "UPDATE prompt_relays SET state = 'expired', outcome_reason = 'expired' WHERE project_id = ? AND relay_id = ?",
              )
              .run(this.#k.projectId, row.relay_id);
            expired.push(row.relay_id);
          }
        return {
          value: expired,
          event: {
            entityType: "prompt_relay",
            entityId: expired[0] ?? "none",
            stateVersion: 0,
            details: { expired },
          },
        };
      },
    );
  }

  /** Startup reconciliation: a row left typing by a daemon that stopped is failed (interrupted). */
  failInterruptedPromptRelays(context: MutationContext): readonly string[] {
    this.#k.assertOpen();
    // Nothing typing: no mutation, so a project that never used the relay sees no new ledger events.
    if (
      this.#k.database
        .prepare(
          "SELECT 1 AS typing FROM prompt_relays WHERE project_id = ? AND state = 'typing' LIMIT 1",
        )
        .get(this.#k.projectId) === undefined
    )
      return [];
    return this.#k.mutate<readonly string[]>(
      context,
      "prompt_relay.interrupted",
      "controller:reconcile",
      {},
      () => {
        const failed: string[] = [];
        for (const row of this.#k.database
          .prepare(
            "SELECT * FROM prompt_relays WHERE project_id = ? AND state = 'typing' ORDER BY sequence",
          )
          .all(this.#k.projectId) as PromptRelayRow[]) {
          this.#endTyping(row, "failed", "interrupted", []);
          failed.push(row.relay_id);
        }
        return {
          value: failed,
          event: {
            entityType: "prompt_relay",
            entityId: failed[0] ?? "none",
            stateVersion: 0,
            details: { failed },
          },
        };
      },
    );
  }
}

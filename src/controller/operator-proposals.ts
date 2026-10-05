import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { sha256 } from "./canonical.js";
import {
  FULL_AUTO_RULE,
  HASH_PREFIX_CHARS,
  MAX_OPERATOR_COMMAND_BYTES,
  MAX_OPERATOR_REASON_BYTES,
  commandHash,
  normalizeCommand,
  normalizeReason,
  prefixGrantProblem,
  sessionRuleGrantId,
  words,
} from "../operator-policy.js";
import {
  decisionNotice,
  endedWithoutRunNotice,
  expiredNotice,
  proposalNoticeToPm,
} from "../operator.js";
import type {
  MutationContext,
  OperatorGrantKind,
  OperatorProposalKind,
  OperatorProposalRecord,
  OperatorProposalState,
} from "./types.js";
import { OPERATOR_PROPOSAL_STATES } from "./types.js";
import { ControllerError } from "./errors.js";
import {
  RESTART_COMMAND_TEXT,
  type OperatorProposalRow,
  type OperatorRunRow,
  type OperatorGrantRow,
  type MutationEvent,
} from "./records.js";
import { safeId, safeText } from "./helpers.js";

export class OperatorProposalsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  operatorRow(proposalId: string): OperatorProposalRow | undefined {
    return this.kernel.database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND proposal_id = ?",
      )
      .get(this.kernel.projectId, proposalId) as
      OperatorProposalRow | undefined;
  }

  operatorRecord(row: OperatorProposalRow): OperatorProposalRecord {
    const run = this.kernel.database
      .prepare(
        "SELECT * FROM operator_runs WHERE project_id = ? AND proposal_id = ?",
      )
      .get(this.kernel.projectId, row.proposal_id) as
      OperatorRunRow | undefined;
    return {
      proposalId: row.proposal_id,
      sequence: row.sequence,
      kind: row.kind,
      command: row.command,
      commandSha: row.command_sha,
      reason: row.reason,
      forceRestart: row.force_restart === 1,
      proposerAgentId: row.proposer_agent_id,
      proposerActorId: row.proposer_actor_id,
      state: row.state,
      autoRule: row.auto_rule,
      decidedByActorId: row.decided_by_actor_id,
      decidedAt: row.decided_at,
      decisionNote: row.decision_note,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
      run:
        run === undefined
          ? null
          : {
              proposalId: run.proposal_id,
              startedAt: run.started_at,
              finishedAt: run.finished_at,
              status: run.status,
              exitCode: run.exit_code,
              durationMs: run.duration_ms,
              outputTail: run.output_tail,
              outputTruncated: run.output_truncated === 1,
              notifiedMessageId: run.notified_message_id,
              pgid: run.pgid,
              leaderStart: run.leader_start,
              orphanClearedAt: run.orphan_cleared_at,
              fullAuto: run.full_auto === 1,
            },
      sessionGrant: this.areas.operatorGrants.grantOfProposal(row.proposal_id),
    };
  }

  operatorProposal(proposalId: string): OperatorProposalRecord | undefined {
    this.kernel.assertOpen();
    safeId(proposalId, "proposal id");
    const row = this.operatorRow(proposalId);
    return row === undefined ? undefined : this.operatorRecord(row);
  }

  /** Proposals newest first, optionally of some states or of one proposer. */
  listOperatorProposals(
    filter: {
      readonly states?: readonly OperatorProposalState[];
      readonly proposerAgentId?: string;
      readonly limit?: number;
    } = {},
  ): readonly OperatorProposalRecord[] {
    this.kernel.assertOpen();
    const states = filter.states ?? OPERATOR_PROPOSAL_STATES;
    const rows = this.kernel.database
      .prepare(
        `SELECT * FROM operator_proposals WHERE project_id = ?
           AND state IN (${states.map(() => "?").join(", ")})
           AND (? IS NULL OR proposer_agent_id = ?)
         ORDER BY sequence DESC LIMIT ?`,
      )
      .all(
        this.kernel.projectId,
        ...states,
        filter.proposerAgentId ?? null,
        filter.proposerAgentId ?? null,
        filter.limit ?? 50,
      ) as OperatorProposalRow[];
    return rows.map((row) => this.operatorRecord(row));
  }

  /** Proposals of an agent that still wait for a decision, a run or the end of a run. */
  pendingOperatorProposalCount(agentId: string): number {
    this.kernel.assertOpen();
    safeId(agentId, "agent id");
    return (
      this.kernel.database
        .prepare(
          "SELECT COUNT(*) AS n FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ? AND state IN ('proposed', 'approved', 'running')",
        )
        .get(this.kernel.projectId, agentId) as { n: number }
    ).n;
  }

  /** Approved proposals in approval order; the worker takes the first. */
  approvedOperatorProposals(): readonly OperatorProposalRecord[] {
    this.kernel.assertOpen();
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'approved' ORDER BY decided_at, sequence",
        )
        .all(this.kernel.projectId) as OperatorProposalRow[]
    ).map((row) => this.operatorRecord(row));
  }

  runningOperatorProposal(): OperatorProposalRecord | undefined {
    this.kernel.assertOpen();
    const row = this.kernel.database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'running' ORDER BY sequence LIMIT 1",
      )
      .get(this.kernel.projectId) as OperatorProposalRow | undefined;
    return row === undefined ? undefined : this.operatorRecord(row);
  }

  operatorEvent(
    proposalId: string,
    fromState: string | undefined,
    toState: string,
    details: Record<string, unknown> = {},
  ): MutationEvent {
    return {
      entityType: "operator_proposal",
      entityId: proposalId,
      stateVersion: 0,
      ...(fromState === undefined ? {} : { fromState }),
      toState,
      details,
    };
  }

  setOperatorState(
    row: OperatorProposalRow,
    state: OperatorProposalState,
    now: string,
  ): void {
    this.kernel.database
      .prepare(
        "UPDATE operator_proposals SET state = ?, updated_at = ? WHERE project_id = ? AND proposal_id = ?",
      )
      .run(state, now, this.kernel.projectId, row.proposal_id);
  }

  operatorRunning(): OperatorProposalRow | undefined {
    return this.kernel.database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'running' ORDER BY sequence LIMIT 1",
      )
      .get(this.kernel.projectId) as OperatorProposalRow | undefined;
  }

  /**
   * Records a proposal from the Operator agent. The command-layer checks that the caller is the designated
   * Operator; here the caller must be an active Developer-kind agent. `autoRule` is the rule the controller
   * matched for an auto-approved command; the row then starts approved.
   */
  proposeOperatorAction(
    context: MutationContext,
    input: {
      readonly kind: OperatorProposalKind;
      readonly command: string;
      readonly reason: string;
      readonly forceRestart?: boolean;
      readonly autoRule?: string | null;
      readonly maxPending: number;
    },
  ): OperatorProposalRecord {
    if (input.kind !== "command" && input.kind !== "restart")
      throw new TypeError("the proposal kind must be command or restart");
    const forceRestart = input.forceRestart === true;
    if (forceRestart && input.kind !== "restart")
      throw new TypeError("only a restart proposal can carry force");
    const autoRule = input.autoRule ?? null;
    if (
      autoRule !== null &&
      input.kind !== "command" &&
      autoRule !== FULL_AUTO_RULE
    )
      throw new TypeError(
        "only a command, or any proposal under full auto, can be auto-approved",
      );
    if (!Number.isInteger(input.maxPending) || input.maxPending < 1)
      throw new TypeError("the pending limit must be a positive integer");
    const commandText =
      input.kind === "restart" ? RESTART_COMMAND_TEXT : input.command;
    const command = normalizeCommand(commandText);
    if (!command.ok)
      throw new TypeError(
        `the command must be non-empty printable ASCII of at most ${MAX_OPERATOR_COMMAND_BYTES} bytes (${command.code})`,
      );
    const reason = normalizeReason(input.reason);
    if (!reason.ok)
      throw new TypeError(
        `the reason must be non-empty printable ASCII of at most ${MAX_OPERATOR_REASON_BYTES} bytes (${reason.code})`,
      );
    const sha = commandHash({
      kind: input.kind,
      command: command.text,
      forceRestart,
    });
    const requestedGrantId = sessionRuleGrantId(autoRule);
    return this.kernel.mutate<OperatorProposalRecord>(
      context,
      "operator.propose",
      "operator:propose",
      {
        kind: input.kind,
        commandSha: sha,
        reasonSha: sha256(reason.text),
        autoRule,
      },
      (actor) => {
        // A session grant that ended between the match and this write no longer approves anything.
        const grant =
          requestedGrantId === undefined
            ? undefined
            : this.areas.operatorGrants
                .activeOperatorGrants()
                .find((candidate) => candidate.grantId === requestedGrantId);
        const effectiveRule =
          requestedGrantId !== undefined && grant === undefined
            ? null
            : (input.autoRule ?? null);
        const agent = this.kernel.agentByActor(actor.actorId);
        if (agent?.kind !== "Developer")
          throw new ControllerError(
            "only an active developer-kind agent proposes an operator action",
          );
        const parties = this.areas.messageNotices.noticeParties();
        if (parties === undefined && effectiveRule === null)
          throw new ControllerError(
            "no_pm: no PM is active to decide this proposal",
          );
        if (
          this.pendingOperatorProposalCount(agent.agent_id) >= input.maxPending
        )
          throw new ControllerError(
            `pending_limit: ${agent.agent_id} already has ${input.maxPending} proposals that wait for a decision or a run`,
          );
        const now = this.kernel.now();
        const sequence = (
          this.kernel.database
            .prepare(
              "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM operator_proposals WHERE project_id = ?",
            )
            .get(this.kernel.projectId) as { next: number }
        ).next;
        const proposalId = `op-${sequence}`;
        this.kernel.database
          .prepare(
            `INSERT INTO operator_proposals(project_id, proposal_id, sequence, kind, command, command_sha, reason, force_restart,
               proposer_agent_id, proposer_actor_id, state, auto_rule, decided_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          )
          .run(
            this.kernel.projectId,
            proposalId,
            sequence,
            input.kind,
            command.text,
            sha,
            reason.text,
            forceRestart ? 1 : 0,
            agent.agent_id,
            actor.actorId,
            effectiveRule === null ? "proposed" : "approved",
            effectiveRule,
            effectiveRule === null ? null : now,
            now,
            now,
          );
        const record = this.operatorRecord(this.operatorRow(proposalId)!);
        this.areas.messageNotices.noticeToPm(
          proposalNoticeToPm(record),
          now,
          true,
        );
        const fullAuto = effectiveRule === FULL_AUTO_RULE;
        return {
          value: record,
          event: this.operatorEvent(proposalId, undefined, record.state, {
            kind: input.kind,
            autoRule: effectiveRule,
            agentId: agent.agent_id,
            ...(fullAuto ? { fullAuto: true } : {}),
          }),
          ...(grant === undefined
            ? {}
            : {
                extraEvents: [
                  {
                    entityType: "operator_grant",
                    entityId: grant.grantId,
                    stateVersion: 0,
                    toState: "used",
                    details: {
                      event: "operator.grant_used",
                      proposalId,
                      commandSha: sha,
                    },
                  },
                ],
              }),
        };
      },
    );
  }

  /**
   * Records the PM's decision. Approval is bound to the exact text: `hash` must be a prefix of at least 12
   * characters of the stored hash, and the force flag is part of that hash. The operator CLI identity may
   * deny but never approve, and no approval is taken while a run is in progress or a stray process group of
   * an abandoned run may still be alive.
   */
  decideOperatorProposal(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly decision: "approve" | "deny";
      readonly hash?: string;
      readonly note?: string;
      readonly proposalTtlMinutes: number;
      /** Approve and allow the same text, or the same leading words, again until the grant ends. */
      readonly session?: {
        readonly kind: OperatorGrantKind;
        readonly text?: string;
        readonly maxMinutes: number;
      };
    },
  ): OperatorProposalRecord {
    safeId(input.proposalId, "proposal id");
    if (input.decision !== "approve" && input.decision !== "deny")
      throw new TypeError("the decision must be approve or deny");
    if (input.session !== undefined && input.decision !== "approve")
      throw new TypeError("a session grant comes only with an approval");
    const note =
      input.note === undefined || input.note === ""
        ? null
        : safeText(input.note, "decision note", 1024, true);
    if (note !== null && Buffer.byteLength(note, "utf8") > 1024)
      throw new TypeError("the decision note must be at most 1024 bytes");
    return this.kernel.mutate<OperatorProposalRecord>(
      context,
      "operator.decide",
      "operator:decide",
      {
        proposalId: input.proposalId,
        decision: input.decision,
        hash: input.hash ?? null,
        note,
        session: input.session ?? null,
      },
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        const isPm = agent?.kind === "PM";
        if (actor.role === "operator") {
          if (input.decision === "approve")
            throw new ControllerError(
              "approve_requires_pm: only an active PM agent approves an operator proposal; the operator may deny, cancel and show",
            );
        } else if (!isPm)
          throw new ControllerError(
            "approve_requires_pm: only an active PM agent decides an operator proposal",
          );
        const row = this.operatorRow(input.proposalId);
        if (row === undefined)
          throw new ControllerError(
            `unknown_proposal: proposal ${input.proposalId} does not exist`,
          );
        if (row.state !== "proposed")
          throw new ControllerError(
            `proposal_not_open: proposal ${row.proposal_id} is ${row.state}`,
          );
        const now = this.kernel.now();
        if (this.#isOlderThan(row.created_at, input.proposalTtlMinutes, now))
          throw new ControllerError(
            `proposal_expired: proposal ${row.proposal_id} waited longer than ${input.proposalTtlMinutes} minutes for a decision`,
          );
        if (input.decision === "approve") {
          if (
            agent?.agent_id === row.proposer_agent_id ||
            actor.actorId === row.proposer_actor_id
          )
            throw new ControllerError(
              "self_approval: the proposer cannot approve its own proposal",
            );
          if (this.operatorRunning() !== undefined)
            throw new ControllerError(
              "run_in_progress: an operator run is in progress; decide again when it ends",
            );
          if (this.areas.operatorRuns.unclearedOperatorOrphans().length > 0)
            throw new ControllerError(
              "orphan_running: a process of an abandoned operator run may still be alive; decide again when the controller has cleared it",
            );
          const hash = input.hash;
          if (
            typeof hash !== "string" ||
            !/^[0-9a-f]{12,64}$/.test(hash) ||
            !row.command_sha.startsWith(hash)
          )
            throw new ControllerError(
              `hash_mismatch: --hash must be at least ${HASH_PREFIX_CHARS} hex characters of the hash in the proposal notice for this exact text`,
            );
        }
        const state: OperatorProposalState =
          input.decision === "approve" ? "approved" : "denied";
        let grant: OperatorGrantRow | undefined;
        if (input.session !== undefined) {
          if (row.kind !== "command")
            throw new ControllerError(
              "session_not_for_restart: a restart is never granted for the session",
            );
          const text =
            input.session.kind === "exact" ? row.command : input.session.text;
          if (text === undefined)
            throw new ControllerError(
              "session_prefix_missing: --session prefix needs the leading words",
            );
          const problem =
            input.session.kind === "exact" ? null : prefixGrantProblem(text);
          if (problem !== null)
            throw new ControllerError(
              `session_prefix_refused: the prefix ${problem}`,
            );
          const own = words(row.command);
          if (
            input.session.kind === "prefix" &&
            !words(text).every((token, index) => own[index] === token)
          )
            throw new ControllerError(
              "session_prefix_mismatch: the prefix must be the start of the command being approved",
            );
          const sequence = (
            this.kernel.database
              .prepare(
                "SELECT COALESCE(MAX(sequence), 0) + 1 AS next FROM operator_grants WHERE project_id = ?",
              )
              .get(this.kernel.projectId) as { next: number }
          ).next;
          const expiresAt = new Date(
            Date.parse(now) + input.session.maxMinutes * 60_000,
          ).toISOString();
          this.kernel.database
            .prepare(
              `INSERT INTO operator_grants(project_id, grant_id, sequence, kind, text, command_sha, created_by, source_proposal_id, created_at, expires_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
            )
            .run(
              this.kernel.projectId,
              `grant-${sequence}`,
              sequence,
              input.session.kind,
              text,
              sha256(text),
              actor.actorId,
              row.proposal_id,
              now,
              expiresAt,
            );
          grant = this.areas.operatorGrants.grantRow(`grant-${sequence}`);
        }
        this.kernel.database
          .prepare(
            `UPDATE operator_proposals SET state = ?, decided_by_actor_id = ?, decided_at = ?, decision_note = ?, updated_at = ?
             WHERE project_id = ? AND proposal_id = ?`,
          )
          .run(
            state,
            actor.actorId,
            now,
            note,
            now,
            this.kernel.projectId,
            row.proposal_id,
          );
        const record = this.operatorRecord(this.operatorRow(row.proposal_id)!);
        this.areas.messageNotices.noticeToAgent(
          row.proposer_agent_id,
          decisionNotice(record, state),
          now,
          false,
        );
        return {
          value: record,
          event: this.operatorEvent(row.proposal_id, "proposed", state, {
            decidedBy: actor.actorId,
          }),
          ...(grant === undefined
            ? {}
            : {
                extraEvents: [
                  {
                    entityType: "operator_grant",
                    entityId: grant.grant_id,
                    stateVersion: 0,
                    toState: "created",
                    details: {
                      event: "operator.grant_created",
                      kind: grant.kind,
                      text: grant.text,
                      expiresAt: grant.expires_at,
                      proposalId: row.proposal_id,
                    },
                  },
                ],
              }),
        };
      },
    );
  }

  /** Withdraws a proposal that has not started: the proposer, the PM or the operator may. */
  cancelOperatorProposal(
    context: MutationContext,
    proposalId: string,
  ): OperatorProposalRecord {
    safeId(proposalId, "proposal id");
    return this.kernel.mutate<OperatorProposalRecord>(
      context,
      "operator.cancel",
      "operator:read",
      { proposalId },
      (actor) => {
        const agent = this.kernel.agentByActor(actor.actorId);
        const row = this.operatorRow(proposalId);
        if (row === undefined)
          throw new ControllerError(
            `unknown_proposal: proposal ${proposalId} does not exist`,
          );
        const isProposer = agent?.agent_id === row.proposer_agent_id;
        if (actor.role !== "operator" && agent?.kind !== "PM" && !isProposer)
          throw new ControllerError(
            "only the proposer, the PM or the operator cancels a proposal",
          );
        if (row.state !== "proposed" && row.state !== "approved")
          throw new ControllerError(
            `not_cancellable: proposal ${proposalId} is ${row.state}; only a proposal that has not started can be cancelled`,
          );
        const now = this.kernel.now();
        this.setOperatorState(row, "cancelled", now);
        const record = this.operatorRecord(this.operatorRow(proposalId)!);
        if (!isProposer)
          this.areas.messageNotices.noticeToAgent(
            row.proposer_agent_id,
            endedWithoutRunNotice(record, "was cancelled; it was not run."),
            now,
            false,
          );
        return {
          value: record,
          event: this.operatorEvent(proposalId, row.state, "cancelled"),
        };
      },
    );
  }

  /** The proposals of an agent that has not started, cancelled because the agent ends or is replaced. The caller owns the transaction. */
  cancelUnstartedOperatorProposalsOf(agentId: string, now: string): void {
    for (const row of this.kernel.database
      .prepare(
        "SELECT * FROM operator_proposals WHERE project_id = ? AND proposer_agent_id = ? AND state IN ('proposed', 'approved')",
      )
      .all(this.kernel.projectId, agentId) as OperatorProposalRow[])
      this.setOperatorState(row, "cancelled", now);
  }

  #isOlderThan(createdAt: string, minutes: number, nowIso: string): boolean {
    return Date.parse(createdAt) + minutes * 60_000 <= Date.parse(nowIso);
  }

  /** Proposals that are past their time to live: undecided ones past the proposal limit, approved ones past the approval limit. Read-only. */
  dueOperatorExpiries(limits: {
    readonly proposalTtlMinutes: number;
    readonly approvalTtlMinutes: number;
  }): readonly string[] {
    this.kernel.assertOpen();
    const now = this.kernel.now();
    return (
      this.kernel.database
        .prepare(
          "SELECT * FROM operator_proposals WHERE project_id = ? AND state IN ('proposed', 'approved') ORDER BY sequence",
        )
        .all(this.kernel.projectId) as OperatorProposalRow[]
    )
      .filter((row) => this.operatorIsStale(row, limits, now))
      .map((row) => row.proposal_id);
  }

  operatorIsStale(
    row: OperatorProposalRow,
    limits: {
      readonly proposalTtlMinutes: number;
      readonly approvalTtlMinutes: number;
    },
    now: string,
  ): boolean {
    if (row.state === "proposed")
      return this.#isOlderThan(row.created_at, limits.proposalTtlMinutes, now);
    return (
      this.#isOlderThan(row.created_at, limits.proposalTtlMinutes, now) ||
      (row.decided_at !== null &&
        this.#isOlderThan(row.decided_at, limits.approvalTtlMinutes, now))
    );
  }

  /** Marks a stale row expired and tells whoever waits: the Operator always, the PM too when an approval lapsed. The caller owns the transaction. */
  expireOperatorRow(row: OperatorProposalRow, now: string): void {
    const behind = this.operatorRunning()?.proposal_id ?? null;
    this.setOperatorState(row, "expired", now);
    const record = this.operatorRecord(this.operatorRow(row.proposal_id)!);
    const body = expiredNotice(record, behind);
    this.areas.messageNotices.noticeToAgent(
      row.proposer_agent_id,
      body,
      now,
      false,
    );
    if (row.state === "approved")
      this.areas.messageNotices.noticeToPm(body, now, false);
  }

  expireOperatorProposals(
    context: MutationContext,
    limits: {
      readonly proposalTtlMinutes: number;
      readonly approvalTtlMinutes: number;
    },
  ): readonly string[] {
    return this.kernel.mutate<readonly string[]>(
      context,
      "operator.expire",
      "controller:reconcile",
      { ...limits },
      () => {
        const now = this.kernel.now();
        const expired: string[] = [];
        for (const row of this.kernel.database
          .prepare(
            "SELECT * FROM operator_proposals WHERE project_id = ? AND state IN ('proposed', 'approved') ORDER BY sequence",
          )
          .all(this.kernel.projectId) as OperatorProposalRow[])
          if (this.operatorIsStale(row, limits, now)) {
            this.expireOperatorRow(row, now);
            expired.push(row.proposal_id);
          }
        return {
          value: expired,
          event: {
            entityType: "operator_proposal",
            entityId: expired[0] ?? "none",
            stateVersion: 0,
            details: { expired },
          },
        };
      },
    );
  }
}

import type { ControllerKernel } from "./kernel.js";
import type { ControllerAreas } from "./areas.js";
import { sha256 } from "./canonical.js";
import { FULL_AUTO_RULE } from "../operator-policy.js";
import { runResultNotice } from "../operator.js";
import type {
  MutationContext,
  OperatorProposalRecord,
  OperatorProposalState,
} from "./types.js";
import { ControllerError } from "./errors.js";
import {
  MAX_OPERATOR_TAIL_BYTES,
  type OperatorProposalRow,
  type OperatorClaimRefusal,
  type OperatorClaim,
  type MutationOutput,
} from "./records.js";
import { safeId } from "./helpers.js";

export class OperatorRunsArea {
  constructor(
    readonly kernel: ControllerKernel,
    readonly areas: ControllerAreas,
  ) {}

  /** Abandoned runs whose process group has not been confirmed gone. */
  unclearedOperatorOrphans(): readonly {
    readonly proposalId: string;
    readonly pgid: number;
    readonly leaderStart: string | null;
  }[] {
    this.kernel.assertOpen();
    return (
      this.kernel.database
        .prepare(
          "SELECT proposal_id, pgid, leader_start FROM operator_runs WHERE project_id = ? AND status = 'abandoned' AND pgid IS NOT NULL AND orphan_cleared_at IS NULL ORDER BY started_at",
        )
        .all(this.kernel.projectId) as {
        proposal_id: string;
        pgid: number;
        leader_start: string | null;
      }[]
    ).map((row) => ({
      proposalId: row.proposal_id,
      pgid: row.pgid,
      leaderStart: row.leader_start,
    }));
  }

  /**
   * The only way a proposal starts to run: approved -> running and the run row in one transaction, so one
   * approval executes at most once and concurrent claims have one winner. A row that is past its time to live
   * becomes expired and is not claimed.
   */
  claimOperatorRun(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly proposalTtlMinutes: number;
      readonly approvalTtlMinutes: number;
    },
  ): OperatorClaim {
    safeId(input.proposalId, "proposal id");
    return this.kernel.mutate<OperatorClaim>(
      context,
      "operator.claim",
      "controller:reconcile",
      { ...input },
      () => {
        const row = this.areas.operatorProposals.operatorRow(input.proposalId);
        const refuse = (
          reason: OperatorClaimRefusal,
        ): MutationOutput<OperatorClaim> => ({
          value: { claimed: false, reason },
          event: {
            entityType: "operator_proposal",
            entityId: input.proposalId,
            stateVersion: 0,
            details: { claimed: false, reason },
          },
        });
        if (row === undefined || row.state !== "approved")
          return refuse("not_approved");
        const now = this.kernel.now();
        // A one-off decision of the PM still runs while the run is paused; a run approved by an
        // auto rule, a session-grant match or full auto does not, and the refusal is recorded.
        const paused = this.areas.pauses.pauseState().run;
        if (paused !== null && row.auto_rule !== null) {
          const detail = `run_paused: running ${row.proposal_id} is refused while the run is paused: ${paused.reason}`;
          this.areas.operatorProposals.setOperatorState(row, "cancelled", now);
          this.areas.messageNotices.noticeToPm(
            `Operator proposal ${row.proposal_id} was approved automatically but not run: the run is paused (${paused.reason}). It is cancelled; the Operator may propose it again after resume.`,
            now,
            false,
          );
          return {
            value: { claimed: false, reason: "run_paused", detail },
            event: this.areas.operatorProposals.operatorEvent(
              row.proposal_id,
              "approved",
              "cancelled",
              {
                claimed: false,
                reason: "run_paused",
                error: detail,
              },
            ),
          };
        }
        if (this.areas.operatorProposals.operatorIsStale(row, input, now)) {
          this.areas.operatorProposals.expireOperatorRow(row, now);
          return refuse("expired");
        }
        if (this.areas.operatorProposals.operatorRunning() !== undefined)
          return refuse("busy");
        this.areas.operatorProposals.setOperatorState(row, "running", now);
        this.kernel.database
          .prepare(
            "INSERT INTO operator_runs(project_id, proposal_id, started_at, status, full_auto) VALUES (?, ?, ?, 'running', ?)",
          )
          .run(
            this.kernel.projectId,
            row.proposal_id,
            now,
            row.auto_rule === FULL_AUTO_RULE ? 1 : 0,
          );
        return {
          value: {
            claimed: true,
            proposal: this.areas.operatorProposals.operatorRecord(
              this.areas.operatorProposals.operatorRow(row.proposal_id)!,
            ),
          },
          event: this.areas.operatorProposals.operatorEvent(
            row.proposal_id,
            "approved",
            "running",
            {
              ...(row.auto_rule === FULL_AUTO_RULE ? { fullAuto: true } : {}),
            },
          ),
        };
      },
    );
  }

  /** The process group of a run that started, so a later start can find a stray one. */
  recordOperatorRunProcess(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly pgid: number;
      readonly leaderStart: string | null;
    },
  ): null {
    safeId(input.proposalId, "proposal id");
    if (!Number.isInteger(input.pgid) || input.pgid <= 1)
      throw new TypeError("the process group must be an integer above 1");
    return this.kernel.mutate<null>(
      context,
      "operator.process",
      "controller:reconcile",
      { ...input },
      () => {
        const changed = this.kernel.database
          .prepare(
            "UPDATE operator_runs SET pgid = ?, leader_start = ? WHERE project_id = ? AND proposal_id = ? AND status = 'running' AND pgid IS NULL",
          )
          .run(
            input.pgid,
            input.leaderStart,
            this.kernel.projectId,
            input.proposalId,
          ).changes;
        if (changed !== 1)
          throw new ControllerError(
            `no running run of ${input.proposalId} can record a process`,
          );
        return {
          value: null,
          event: {
            entityType: "operator_proposal",
            entityId: input.proposalId,
            stateVersion: 0,
            details: { pgid: input.pgid },
          },
        };
      },
    );
  }

  /** Ends a run with its result, tells the Operator (and the PM for an auto-approved command) and moves the proposal to its final state. */
  finishOperatorRun(
    context: MutationContext,
    input: {
      readonly proposalId: string;
      readonly status: "ok" | "failed" | "timeout" | "error";
      readonly exitCode: number | null;
      readonly durationMs: number;
      readonly outputTail: string;
      readonly truncated: boolean;
    },
  ): OperatorProposalRecord {
    safeId(input.proposalId, "proposal id");
    if (Buffer.byteLength(input.outputTail, "utf8") > MAX_OPERATOR_TAIL_BYTES)
      throw new TypeError(
        `the output tail must be at most ${MAX_OPERATOR_TAIL_BYTES} bytes`,
      );
    return this.kernel.mutate<OperatorProposalRecord>(
      context,
      "operator.finish",
      "controller:reconcile",
      {
        proposalId: input.proposalId,
        status: input.status,
        exitCode: input.exitCode,
        durationMs: input.durationMs,
        tailSha: sha256(input.outputTail),
      },
      () => {
        const row = this.areas.operatorProposals.operatorRow(input.proposalId);
        if (row?.state !== "running")
          throw new ControllerError(
            `proposal ${input.proposalId} is not running`,
          );
        const now = this.kernel.now();
        this.kernel.database
          .prepare(
            `UPDATE operator_runs SET status = ?, finished_at = ?, exit_code = ?, duration_ms = ?, output_tail = ?, output_truncated = ?
             WHERE project_id = ? AND proposal_id = ? AND status = 'running'`,
          )
          .run(
            input.status,
            now,
            input.exitCode,
            Math.max(0, Math.round(input.durationMs)),
            input.outputTail,
            input.truncated ? 1 : 0,
            this.kernel.projectId,
            input.proposalId,
          );
        const state: OperatorProposalState =
          input.status === "ok"
            ? "finished"
            : input.status === "timeout"
              ? "timeout"
              : "failed";
        this.areas.operatorProposals.setOperatorState(row, state, now);
        const record = this.areas.operatorProposals.operatorRecord(
          this.areas.operatorProposals.operatorRow(row.proposal_id)!,
        );
        this.#announceOperatorRun(record, now);
        return {
          value: this.areas.operatorProposals.operatorRecord(
            this.areas.operatorProposals.operatorRow(row.proposal_id)!,
          ),
          event: this.areas.operatorProposals.operatorEvent(
            row.proposal_id,
            "running",
            state,
            {
              status: input.status,
              exitCode: input.exitCode,
              ...(row.auto_rule === FULL_AUTO_RULE ? { fullAuto: true } : {}),
            },
          ),
        };
      },
    );
  }

  /** Sends the result of an ended run once. The caller owns the transaction. */
  #announceOperatorRun(record: OperatorProposalRecord, now: string): void {
    if (record.run === null) return;
    const body = runResultNotice(record, record.run);
    const messageId = this.areas.messageNotices.noticeToAgent(
      record.proposerAgentId,
      body,
      now,
      false,
    );
    if (record.autoRule !== null)
      this.areas.messageNotices.noticeToPm(body, now, false);
    if (messageId !== null)
      this.kernel.database
        .prepare(
          "UPDATE operator_runs SET notified_message_id = ? WHERE project_id = ? AND proposal_id = ? AND notified_message_id IS NULL",
        )
        .run(messageId, this.kernel.projectId, record.proposalId);
  }

  /**
   * At startup: a run that was running when the controller stopped is abandoned and never run again. A
   * restart row whose plan file exists is left alone when `skipRestartsWithPlan` says so; its result is
   * ingested by the restart recovery. Returns the abandoned runs that recorded a process group.
   */
  abandonRunningOperatorRuns(
    context: MutationContext,
    options: {
      readonly skipRestartsWithPlan?: (proposalId: string) => boolean;
    } = {},
  ): readonly {
    readonly proposalId: string;
    readonly pgid: number | null;
    readonly leaderStart: string | null;
  }[] {
    return this.kernel.mutate(
      context,
      "operator.abandon",
      "controller:reconcile",
      {},
      () => {
        const now = this.kernel.now();
        const abandoned: {
          proposalId: string;
          pgid: number | null;
          leaderStart: string | null;
        }[] = [];
        for (const row of this.kernel.database
          .prepare(
            "SELECT * FROM operator_proposals WHERE project_id = ? AND state = 'running' ORDER BY sequence",
          )
          .all(this.kernel.projectId) as OperatorProposalRow[]) {
          if (
            row.kind === "restart" &&
            options.skipRestartsWithPlan?.(row.proposal_id) === true
          )
            continue;
          const run = this.kernel.database
            .prepare(
              "SELECT pgid, leader_start, started_at FROM operator_runs WHERE project_id = ? AND proposal_id = ?",
            )
            .get(this.kernel.projectId, row.proposal_id) as
            | {
                pgid: number | null;
                leader_start: string | null;
                started_at: string;
              }
            | undefined;
          this.kernel.database
            .prepare(
              `UPDATE operator_runs SET status = 'abandoned', finished_at = ?, duration_ms = ?
               WHERE project_id = ? AND proposal_id = ? AND status = 'running'`,
            )
            .run(
              now,
              Math.max(0, Date.parse(now) - Date.parse(run?.started_at ?? now)),
              this.kernel.projectId,
              row.proposal_id,
            );
          this.areas.operatorProposals.setOperatorState(row, "abandoned", now);
          this.#announceOperatorRun(
            this.areas.operatorProposals.operatorRecord(
              this.areas.operatorProposals.operatorRow(row.proposal_id)!,
            ),
            now,
          );
          abandoned.push({
            proposalId: row.proposal_id,
            pgid: run?.pgid ?? null,
            leaderStart: run?.leader_start ?? null,
          });
        }
        return {
          value: abandoned,
          event: {
            entityType: "operator_proposal",
            entityId: abandoned[0]?.proposalId ?? "none",
            stateVersion: 0,
            details: { abandoned: abandoned.map((a) => a.proposalId) },
          },
        };
      },
    );
  }

  /** Records that the process group of an abandoned run is gone (or was never ours to signal). */
  clearOperatorOrphan(context: MutationContext, proposalId: string): null {
    safeId(proposalId, "proposal id");
    return this.kernel.mutate<null>(
      context,
      "operator.orphan_clear",
      "controller:reconcile",
      { proposalId },
      () => {
        this.kernel.database
          .prepare(
            "UPDATE operator_runs SET orphan_cleared_at = ? WHERE project_id = ? AND proposal_id = ? AND status = 'abandoned' AND orphan_cleared_at IS NULL",
          )
          .run(this.kernel.now(), this.kernel.projectId, proposalId);
        return {
          value: null,
          event: {
            entityType: "operator_proposal",
            entityId: proposalId,
            stateVersion: 0,
            details: { orphanCleared: true },
          },
        };
      },
    );
  }
}

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { openSqlite, type Database } from "../src/controller/sqlite.js";
import type {
  CapstanConfig,
  ResolvedOperator,
} from "../src/config/capstan-config.js";
import type { RunOptions, RunResult } from "../src/command-runner.js";
import type { OperatorProposalRecord } from "../src/controller/types.js";
import {
  createOperatorService,
  type OperatorService,
  type OperatorServiceOptions,
} from "../src/operator.js";
import { close, ctx, harness, type Harness } from "./harness.js";

/** The role of the harness's developer agent, which these tests make the Operator. */
export const OPERATOR_ROLE = "developer";

export function operatorConfig(
  overrides: Partial<ResolvedOperator> = {},
): ResolvedOperator {
  return {
    configured: true,
    enabled: true,
    role: OPERATOR_ROLE,
    autoApprove: [],
    autoApprovePrefix: [],
    timeoutSeconds: 300,
    maxTimeoutSeconds: 1800,
    outputTailBytes: 8192,
    proposalTtlMinutes: 60,
    approvalTtlMinutes: 10,
    maxPendingProposals: 5,
    countTowardWorkerLimit: false,
    restartHealthTimeoutSeconds: 60,
    restartIdleWaitSeconds: 120,
    sessionGrantMaxMinutes: 60,
    fullAutoDefaultMinutes: 30,
    fullAutoMaxMinutes: 120,
    ...overrides,
  };
}

/** The parts of the configuration the command layer reads. */
export function commandConfig(
  operator: ResolvedOperator,
  architectRole: string | null = null,
): CapstanConfig {
  return {
    architect: {
      enabled: architectRole !== null,
      role: architectRole ?? "architect",
    },
    operator,
    roles: [],
    hosts: [],
  } as unknown as CapstanConfig;
}

export const OK_RESULT: RunResult = {
  status: "ok",
  exitCode: 0,
  signal: null,
  durationMs: 5,
  outputTail: "done",
  truncated: false,
};

export interface FakeClock {
  readonly now: () => Date;
  set(ms: number): void;
  advance(ms: number): void;
}

export function fakeClock(start = Date.parse("2026-01-01T00:00:00.000Z")) {
  let current = start;
  const clock: FakeClock = {
    now: () => new Date(current),
    set(ms) {
      current = ms;
    },
    advance(ms) {
      current += ms;
    },
  };
  return clock;
}

export interface OperatorWorld {
  readonly h: Harness;
  readonly clock: FakeClock;
  readonly projectRoot: string;
  readonly config: ResolvedOperator;
  readonly service: OperatorService;
  /** Commands the fake runner was asked to run. */
  readonly runs: RunOptions[];
  /** Raw access to the ledger for trigger tests. */
  raw<T>(use: (database: Database) => T): T;
  /** Proposes as the operator agent and returns the record. */
  propose(
    command: string,
    reason?: string,
    forceRestart?: boolean,
  ): OperatorProposalRecord;
  /** Approves as the PM with the right hash, straight in the ledger: the worker does not start. */
  approve(proposal: OperatorProposalRecord): OperatorProposalRecord;
  stop(): Promise<void>;
}

export async function operatorWorld(
  options: {
    operator?: Partial<ResolvedOperator>;
    /** Turns `[architect]` on with this role. */
    architectRole?: string;
    runCommand?: (run: RunOptions) => Promise<RunResult>;
    service?: Partial<OperatorServiceOptions>;
  } = {},
): Promise<OperatorWorld> {
  const clock = fakeClock();
  const config = operatorConfig(options.operator);
  const projectRoot = mkdtempSync(path.join(tmpdir(), "capstan-operator-"));
  const runs: RunOptions[] = [];
  const holder: { service?: OperatorService } = {};
  const h = await harness({
    clock: clock.now,
    commands: {
      config: commandConfig(config, options.architectRole ?? null),
      operator: new Proxy({} as OperatorService, {
        get: (_target, key) => Reflect.get(holder.service!, key) as unknown,
      }),
    },
  });
  const service = createOperatorService({
    core: h.core,
    config,
    controllerCredential: h.owner,
    projectRoot,
    environment: () => ({ PATH: process.env.PATH ?? "/usr/bin:/bin" }),
    monotonicNow: () => clock.now().getTime(),
    runCommand:
      options.runCommand ??
      (async (run) => {
        runs.push(run);
        return OK_RESULT;
      }),
    ...options.service,
  });
  holder.service = service;
  const world: OperatorWorld = {
    h,
    clock,
    projectRoot,
    config,
    service,
    runs,
    raw(use) {
      const database = openSqlite(
        path.join(h.stateDirectory, "controller.sqlite"),
      );
      try {
        return use(database);
      } finally {
        database.close();
      }
    },
    propose(command, reason = "check it", forceRestart = false) {
      return service.propose(h.developer.credential, {
        kind: "command",
        command,
        reason,
        forceRestart,
      });
    },
    approve(proposal) {
      return h.core.decideOperatorProposal(ctx(h.core, h.pm.credential), {
        proposalId: proposal.proposalId,
        decision: "approve",
        hash: proposal.commandSha.slice(0, 12),
        proposalTtlMinutes: config.proposalTtlMinutes,
      });
    },
    async stop() {
      await service.stop();
      rmSync(projectRoot, { recursive: true, force: true });
      await close(h);
    },
  };
  return world;
}

export { ctx };

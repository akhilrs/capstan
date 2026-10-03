import assert from "node:assert/strict";

// No test may reach a real Herdr session: the daemon stays out of it.
process.env.CAPSTAN_LAUNCH = "off";
import { mkdirSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { callDaemon } from "../src/client.js";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import { RESTART_COMMAND_TEXT } from "../src/controller/core.js";
import { runDaemon, type LogEntry } from "../src/daemon.js";
import type { Notifier } from "../src/notifier.js";
import { distHash, knownGoodBuild, restartDirectory } from "../src/restart.js";
import { ctx, harness, type Harness } from "./harness.js";
import { StubAdapter } from "./launcher-stubs.js";
import { operatorConfig } from "./operator-harness.js";
import {
  fakeProject,
  waitFor,
  writeFakeBuild,
  type FakeProject,
} from "./restart-fakes.js";

const notifier: Notifier = {
  send: async () => [{ channel: "fallback", ok: true }],
  write: () => undefined,
};

function configFor(operatorEnabled: boolean): CapstanConfig {
  const roleOf = (name: string, kind: string, hash: string) =>
    Object.defineProperty(
      {
        name,
        kind,
        host: "claude",
        model: null,
        permissionMode: "default",
        allow: [],
        deny: [],
        hooks: "off",
        prompt: { source: "none", path: null, hash: null },
        configHash: hash.repeat(64),
      },
      "promptText",
      { value: null, enumerable: false },
    );
  return {
    schemaVersion: 1,
    projectName: null,
    herdrSession: "unused",
    notifications: { herdr: false, fallback: true },
    timers: {
      maxDeferralSeconds: 120,
      pmAckTimeoutSeconds: 600,
      pmNotifyAfterSeconds: 300,
      notifyIntervalSeconds: 600,
      stallAfterSeconds: 900,
      workerAckTimeoutSeconds: 600,
      findingCheckSeconds: 1800,
    },
    limits: { maxWorkers: 3 },
    ledger: { keepMigrationBackups: 3 },
    layout: {
      spawn: "tab",
      split: "auto",
      minPaneColumns: 60,
      minPaneRows: 12,
    },
    env: { pass: [] },
    hosts: [
      {
        name: "claude",
        kind: "claude",
        command: "claude",
        shellCommandTimeoutSeconds: 120,
        waitTimeoutSeconds: 45,
      },
    ],
    roles: [roleOf("pm", "PM", "a"), roleOf("developer", "Developer", "b")],
    operator: operatorConfig({ enabled: operatorEnabled }),
    architect: { enabled: false, role: "architect" },
  } as unknown as CapstanConfig;
}

interface Running {
  readonly log: LogEntry[];
  readonly socket: string;
  stop(): Promise<void>;
}

/** Runs the daemon on the harness's state directory after the harness's own server and core are closed. */
async function startDaemon(
  h: Harness,
  project: FakeProject,
  options: { operator: boolean; settleMs?: number },
): Promise<Running> {
  await h.server.close();
  h.core.close();
  const log: LogEntry[] = [];
  let ready!: () => void;
  const up = new Promise<void>((resolve) => {
    ready = resolve;
  });
  const done = runDaemon({
    stateDirectory: h.stateDirectory,
    project: h.info,
    workspaceRoot: project.root,
    log: (entry) => log.push(entry),
    announce: (event) => {
      if (event.event === "ready") ready();
    },
    capstan: configFor(options.operator),
    adapter: new StubAdapter(),
    notifier,
    cliPath: path.join(project.distDir, "src", "cli.js"),
    tickMs: 500,
    restart: {
      distDir: project.distDir,
      knownGoodSettleMs: options.settleMs ?? 150,
      resultPollMs: 600_000,
    },
  });
  await up;
  const socket = path.join(h.stateDirectory, "control.sock");
  return {
    log,
    socket,
    async stop() {
      await callDaemon(socket, h.owner, "shutdown", [], 5000).catch(
        () => undefined,
      );
      await done.catch(() => undefined);
    },
  };
}

function seedRunning(
  h: Harness,
  kind: "restart" | "command",
): { proposalId: string } {
  const command = kind === "restart" ? RESTART_COMMAND_TEXT : "echo hi";
  const proposal = h.core.proposeOperatorAction(
    ctx(h.core, h.developer.credential),
    { kind, command, reason: "seed", forceRestart: false, maxPending: 5 },
  );
  h.core.decideOperatorProposal(ctx(h.core, h.pm.credential), {
    proposalId: proposal.proposalId,
    decision: "approve",
    hash: proposal.commandSha.slice(0, 12),
    proposalTtlMinutes: 60,
  });
  const claim = h.core.claimOperatorRun(ctx(h.core, h.owner), {
    proposalId: proposal.proposalId,
    proposalTtlMinutes: 60,
    approvalTtlMinutes: 10,
  });
  assert.equal(claim.claimed, true);
  return { proposalId: proposal.proposalId };
}

async function states(h: Harness, stateDirectory: string): Promise<string[]> {
  const { default: Database } = await import("better-sqlite3");
  const database = new Database(
    path.join(stateDirectory, "controller.sqlite"),
    {
      readonly: true,
    },
  );
  try {
    return (
      database
        .prepare("SELECT state FROM operator_proposals ORDER BY sequence")
        .all() as { state: string }[]
    ).map((row) => row.state);
  } finally {
    database.close();
    void h;
  }
}

function writePlanAndResult(
  stateDirectory: string,
  proposalId: string,
  result: object | null,
): void {
  const directory = restartDirectory(stateDirectory, proposalId);
  mkdirSync(directory, { recursive: true });
  writeFileSync(path.join(directory, "plan.json"), "{}");
  if (result !== null)
    writeFileSync(path.join(directory, "result.json"), JSON.stringify(result));
}

test("startup order: the result of a restart is ingested before abandon runs, and no abandoned row exists for a restart that had a plan", async () => {
  const h = await harness();
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  try {
    const { proposalId } = seedRunning(h, "restart");
    writePlanAndResult(h.stateDirectory, proposalId, { outcome: "ok", pid: 1 });
    // The helper-process check must not matter: the result file alone ends the run.
    const running = await startDaemon(h, project, { operator: true });
    try {
      assert.deepEqual(await states(h, h.stateDirectory), ["finished"]);
      assert.ok(
        !running.log.some((entry) => /abandon/.test(entry.command)),
        "nothing was abandoned",
      );
    } finally {
      await running.stop();
    }
  } finally {
    await project.cleanup();
  }
});

test("startup order: a rolled-back result ends the run failed, never abandoned", async () => {
  const h = await harness();
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  try {
    const { proposalId } = seedRunning(h, "restart");
    writePlanAndResult(h.stateDirectory, proposalId, {
      outcome: "rolled_back",
      reason: "no ping",
    });
    const running = await startDaemon(h, project, { operator: true });
    try {
      assert.deepEqual(await states(h, h.stateDirectory), ["failed"]);
    } finally {
      await running.stop();
    }
  } finally {
    await project.cleanup();
  }
});

test("a running restart with a plan but no result and no live helper is abandoned; a running command is abandoned too", async () => {
  for (const kind of ["restart", "command"] as const) {
    const h = await harness();
    const project = fakeProject();
    writeFakeBuild(project.distDir, "good");
    try {
      const { proposalId } = seedRunning(h, kind);
      if (kind === "restart")
        writePlanAndResult(h.stateDirectory, proposalId, null);
      const running = await startDaemon(h, project, { operator: true });
      try {
        assert.deepEqual(await states(h, h.stateDirectory), ["abandoned"]);
      } finally {
        await running.stop();
      }
    } finally {
      await project.cleanup();
    }
  }
});

test("the daemon takes the known-good snapshot only after the settle time", async () => {
  const h = await harness();
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  try {
    const running = await startDaemon(h, project, {
      operator: true,
      settleMs: 1500,
    });
    try {
      assert.equal(knownGoodBuild(h.stateDirectory).exists(), false);
      await waitFor(() => knownGoodBuild(h.stateDirectory).exists(), 8000);
      assert.equal(
        distHash(path.join(h.stateDirectory, "known-good", "dist")),
        distHash(project.distDir),
      );
      assert.ok(
        running.log.some(
          (entry) => entry.command === "daemon:known_good_saved",
        ),
      );
    } finally {
      await running.stop();
    }
  } finally {
    await project.cleanup();
  }
});

test("a build run inside the settle window is never snapshotted by the daemon", async () => {
  const h = await harness();
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  try {
    const running = await startDaemon(h, project, {
      operator: true,
      settleMs: 400,
    });
    try {
      writeFileSync(
        path.join(project.distDir, "src", "later.js"),
        "// build\n",
      );
      await waitFor(
        () =>
          running.log.some((entry) => entry.detail?.reason !== undefined) ||
          running.log.some(
            (entry) => entry.command === "daemon:known_good_skipped",
          ),
      );
      assert.equal(knownGoodBuild(h.stateDirectory).exists(), false);
    } finally {
      await running.stop();
    }
  } finally {
    await project.cleanup();
  }
});

test("without [operator] enabled no restart code starts: no restart directory, no snapshot, and op is not configured", async () => {
  const h = await harness();
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  try {
    const running = await startDaemon(h, project, {
      operator: false,
      settleMs: 100,
    });
    try {
      const op = await callDaemon(running.socket, h.owner, "op", ["show", "x"]);
      assert.match(JSON.stringify(op), /not_configured/);
      await new Promise((resolve) => setTimeout(resolve, 600));
      assert.equal(knownGoodBuild(h.stateDirectory).exists(), false);
      assert.deepEqual(
        readdirSync(h.stateDirectory).filter((name) =>
          /restart|known-good/.test(name),
        ),
        [],
      );
      assert.ok(
        !running.log.some((entry) => /known_good|restart/.test(entry.command)),
      );
    } finally {
      await running.stop();
    }
  } finally {
    await project.cleanup();
  }
});

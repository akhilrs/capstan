import assert from "node:assert/strict";
import {
  execFileSync,
  spawn,
  type spawn as spawnType,
} from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { test } from "node:test";
import type { ResolvedOperator } from "../src/config/capstan-config.js";
import { pingDaemon } from "../src/client.js";
import { RESTART_COMMAND_TEXT } from "../src/controller/core.js";
import type { OperatorProposalRecord } from "../src/controller/types.js";
import {
  busySnapshot,
  createRestartCoordinator,
  distHash,
  knownGoodBuild,
  recoverRestartResults,
  restartDirectory,
  restartNoticeToPm,
  restartRunReport,
  scheduleKnownGoodSnapshot,
  skipRestartsWithLivePlan,
  type RestartCoordinatorOptions,
} from "../src/restart.js";
import type { RestartResult } from "../src/restart-helper.js";
import { call, ctx } from "./harness.js";
import { operatorWorld, type OperatorWorld } from "./operator-harness.js";
import {
  CREDENTIAL,
  alive,
  fakeProject,
  stopFakeController,
  waitFor,
  writeFakeBuild,
  type FakeProject,
} from "./restart-fakes.js";

const HELPER_SOURCE = path.resolve(
  import.meta.dirname,
  "..",
  "src",
  "restart-helper.js",
);

interface Recorded {
  readonly command: string;
  readonly args: readonly string[];
}

interface Setup {
  readonly project: FakeProject;
  readonly world: OperatorWorld;
  readonly spawned: Recorded[];
  readonly stops: number[];
  readonly busy: { value: string[] };
  readonly coordinator: ReturnType<typeof createRestartCoordinator>;
  propose(force?: boolean): OperatorProposalRecord;
  runRestart(force?: boolean): Promise<OperatorProposalRecord>;
  pmMessages(): string[];
  operatorMessages(): string[];
  cleanup(): Promise<void>;
}

async function setup(
  options: {
    knownGood?: boolean;
    idleWaitSeconds?: number;
    operator?: Partial<ResolvedOperator>;
    realSpawnWithNode?: string;
  } = {},
): Promise<Setup> {
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  if (options.knownGood !== false)
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
  const spawned: Recorded[] = [];
  const stops: number[] = [];
  const busy = { value: [] as string[] };
  const holder: { world?: OperatorWorld } = {};
  let clock = 0;
  const coordinatorOptions: RestartCoordinatorOptions = {
    stateDir: project.stateDir,
    projectRoot: project.root,
    distDir: project.distDir,
    helperSource: HELPER_SOURCE,
    node: options.realSpawnWithNode ?? process.execPath,
    argv: [path.join(project.distDir, "src", "cli.js"), "daemon"],
    socketPath: project.socketPath,
    pidPath: project.pidPath,
    logPath: project.logPath,
    credentialFile: project.keyPath,
    healthTimeoutSeconds: 3,
    idleWaitSeconds: options.idleWaitSeconds ?? 5,
    busy: () => busy.value,
    notifyPm: (body) => {
      const world = holder.world!;
      world.h.core.enqueueMessage(ctx(world.h.core, world.h.owner), {
        recipientAgentId: world.h.pm.agentId,
        body,
      });
    },
    requestStop: () => {
      stops.push(Date.now());
    },
    spawn: ((command: string, args: readonly string[]) => {
      spawned.push({ command, args });
      return { pid: 4_194_000, unref() {}, once() {} };
    }) as unknown as typeof spawnType,
    now: () => clock,
    sleep: async (ms) => {
      clock += ms;
    },
  };
  if (options.realSpawnWithNode !== undefined)
    delete (coordinatorOptions as { spawn?: unknown }).spawn;
  const coordinator = createRestartCoordinator(coordinatorOptions);
  const world = await operatorWorld({
    ...(options.operator === undefined ? {} : { operator: options.operator }),
    service: {
      restart: (proposal) => coordinator.run(proposal),
      restartPreflight: () => coordinator.preflight(),
      notifyPm: coordinatorOptions.notifyPm,
    },
  });
  holder.world = world;
  const propose = (force = false): OperatorProposalRecord =>
    world.service.propose(world.h.developer.credential, {
      kind: "restart",
      command: RESTART_COMMAND_TEXT,
      reason: "load the new build",
      forceRestart: force,
    });
  return {
    project,
    world,
    spawned,
    stops,
    busy,
    coordinator,
    propose,
    async runRestart(force = false) {
      const proposal = world.approve(propose(force));
      await world.service.drain();
      return world.h.core.operatorProposal(proposal.proposalId)!;
    },
    pmMessages: () =>
      world.h.core.messagesFor(world.h.pm.agentId).map((m) => m.body),
    operatorMessages: () =>
      world.h.core.messagesFor(world.h.developer.agentId).map((m) => m.body),
    async cleanup() {
      await world.stop();
      await project.cleanup();
    },
  };
}

test("a restart proposal is never auto-approved even if listed, and only an active PM approves it", async () => {
  const s = await setup({
    operator: { autoApprove: ["git status"], autoApprovePrefix: ["restart"] },
  });
  try {
    const proposal = s.propose();
    assert.equal(proposal.state, "proposed");
    assert.equal(proposal.autoRule, null);
    const hash = proposal.commandSha.slice(0, 12);
    const cli = await call(s.world.h, s.world.h.owner, "op", [
      "decide",
      proposal.proposalId,
      "approve",
      "--hash",
      hash,
    ]);
    assert.equal(cli.ok, false);
    assert.match(JSON.stringify(cli), /approve_requires_pm/);
    const forced = s.propose(true);
    assert.notEqual(
      forced.commandSha,
      proposal.commandSha,
      "the hash covers the force flag",
    );
    const approved = await call(s.world.h, s.world.h.pm.credential, "op", [
      "decide",
      proposal.proposalId,
      "approve",
      "--hash",
      hash,
    ]);
    assert.equal(approved.ok, true, JSON.stringify(approved));
  } finally {
    await s.cleanup();
  }
});

test("busySnapshot lists a launcher operation, a started review, a non-terminal integration and an unacked delivery; idle is empty", () => {
  const core = (
    startedReviews: number,
    nonTerminalIntegrations: number,
    unackedDeliveries: number,
  ) => ({
    busyIndicators: () => ({
      startedReviews,
      nonTerminalIntegrations,
      unackedDeliveries,
    }),
  });
  assert.deepEqual(
    busySnapshot(core(0, 0, 0), { inFlightOperations: () => 0 }),
    [],
  );
  assert.deepEqual(busySnapshot(core(0, 0, 0), undefined), []);
  assert.match(
    busySnapshot(core(0, 0, 0), { inFlightOperations: () => 2 }).join(";"),
    /2 launcher spawn, release or replace in flight/,
  );
  assert.match(busySnapshot(core(1, 0, 0), undefined).join(";"), /review/);
  assert.match(
    busySnapshot(core(0, 3, 0), undefined).join(";"),
    /3 integration/,
  );
  assert.match(busySnapshot(core(0, 0, 4), undefined).join(";"), /4 delivery/);
});

test("busySnapshot on the real ledger sees an unacked delivery and ignores a running operator run", async () => {
  const s = await setup();
  try {
    const { h } = s.world;
    assert.deepEqual(busySnapshot(h.core, undefined), []);
    h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.developer.agentId,
      body: "hello",
    });
    // A queued message is not in flight; a sent one is.
    assert.deepEqual(busySnapshot(h.core, undefined), []);
    const running = s.world.approve(s.world.propose("echo hi"));
    h.core.claimOperatorRun(ctx(h.core, h.owner), {
      proposalId: running.proposalId,
      proposalTtlMinutes: 60,
      approvalTtlMinutes: 10,
    });
    assert.deepEqual(
      busySnapshot(h.core, undefined),
      [],
      "an operator run does not count: the restart would wait on itself",
    );
  } finally {
    await s.cleanup();
  }
});

test("with a busy worker the restart does not run: after the idle wait it fails with the busy list and the PM is told", async () => {
  const s = await setup({ idleWaitSeconds: 3 });
  try {
    s.busy.value = ["1 review(s) in state started"];
    const result = await s.runRestart();
    assert.equal(result.state, "failed");
    assert.match(result.run!.outputTail, /^busy: /);
    assert.match(result.run!.outputTail, /1 review\(s\) in state started/);
    assert.match(result.run!.outputTail, /--force/);
    assert.equal(s.spawned.length, 0, "no helper was started");
    assert.equal(s.stops.length, 0, "the controller was not stopped");
    assert.equal(existsSync(path.join(s.project.stateDir, "restart")), false);
    assert.ok(
      s
        .pmMessages()
        .some((body) => /refused \(busy\)/.test(body) && /review/.test(body)),
    );
    assert.ok(s.operatorMessages().some((body) => /busy: /.test(body)));
  } finally {
    await s.cleanup();
  }
});

test("a restart that becomes idle inside the wait proceeds without --force", async () => {
  const s = await setup({ idleWaitSeconds: 10 });
  try {
    s.busy.value = ["1 integration(s) not yet terminal"];
    let polls = 0;
    const original = s.busy;
    Object.defineProperty(original, "value", {
      get() {
        polls += 1;
        return polls > 3 ? [] : ["1 integration(s) not yet terminal"];
      },
    });
    const result = await s.runRestart();
    assert.equal(result.state, "running");
    assert.equal(s.spawned.length, 1);
  } finally {
    await s.cleanup();
  }
});

test("a forced restart proceeds while busy, writes the plan, starts the helper detached and stops the controller; the run stays running until the result is ingested", async () => {
  const s = await setup();
  try {
    s.busy.value = ["1 review(s) in state started"];
    const result = await s.runRestart(true);
    assert.equal(result.state, "running");
    assert.equal(result.run!.status, "running");
    assert.equal(s.spawned.length, 1);
    assert.equal(s.stops.length, 1);
    const directory = restartDirectory(s.project.stateDir, result.proposalId);
    const copied = path.join(directory, "helper.mjs");
    assert.equal(s.spawned[0]!.args[0], copied);
    assert.equal(
      readFileSync(copied, "utf8"),
      readFileSync(HELPER_SOURCE, "utf8"),
      "the helper is copied out of dist",
    );
    const plan = JSON.parse(
      readFileSync(path.join(directory, "plan.json"), "utf8"),
    );
    assert.equal(plan.id, result.proposalId);
    assert.equal(plan.pid, process.pid);
    assert.equal(plan.socketPath, s.project.socketPath);
    assert.equal(plan.knownGoodPath, s.project.knownGoodPath);
    assert.equal(plan.healthTimeoutSeconds, 3);
    assert.equal(plan.credentialFile, s.project.keyPath);
    assert.equal(plan.cwd, s.project.root);
    assert.equal(
      readFileSync(path.join(directory, "helper.pid"), "utf8").trim(),
      "4194000",
    );
  } finally {
    await s.cleanup();
  }
});

test("with all idle a restart proceeds without --force", async () => {
  const s = await setup();
  try {
    const result = await s.runRestart();
    assert.equal(result.state, "running");
    assert.equal(s.spawned.length, 1);
  } finally {
    await s.cleanup();
  }
});

test("a restart is refused with no_known_good at propose time and at run time, and no process is spawned", async () => {
  const s = await setup({ knownGood: false });
  try {
    assert.throws(
      () => s.propose(),
      (error: unknown) => (error as { code?: string }).code === "no_known_good",
    );
    const refused = await call(
      s.world.h,
      s.world.h.developer.credential,
      "op",
      ["propose", "--restart", "load the build"],
    );
    assert.equal(refused.ok, false);
    assert.match(JSON.stringify(refused), /no_known_good/);

    // Run time: the snapshot exists when the proposal is made and is gone when it runs.
    knownGoodBuild(s.project.stateDir).snapshot(
      s.project.distDir,
      s.project.root,
    );
    const proposal = s.world.approve(s.propose());
    rmSync(s.project.knownGoodPath, { recursive: true });
    await s.world.service.drain();
    const result = s.world.h.core.operatorProposal(proposal.proposalId)!;
    assert.equal(result.state, "failed");
    assert.match(result.run!.outputTail, /^no_known_good: /);
    assert.equal(s.spawned.length, 0);
    assert.equal(s.stops.length, 0);
    assert.ok(s.pmMessages().some((body) => /no_known_good/.test(body)));
  } finally {
    await s.cleanup();
  }
});

test("changed dependencies put a warning in front of the PM", async () => {
  const s = await setup();
  try {
    writeFileSync(path.join(s.project.root, "package-lock.json"), "{}\n");
    assert.deepEqual(
      knownGoodBuild(s.project.stateDir).changedDependencies(s.project.root),
      ["package-lock.json"],
    );
    s.propose();
    assert.ok(
      s
        .pmMessages()
        .some((body) =>
          body.includes(
            "dependencies changed since the known-good build: rollback may not start",
          ),
        ),
    );
  } finally {
    await s.cleanup();
  }
});

function writeResult(
  project: FakeProject,
  proposalId: string,
  result: RestartResult,
  options: { plan?: boolean } = {},
): void {
  const directory = restartDirectory(project.stateDir, proposalId);
  mkdirSync(directory, { recursive: true });
  if (options.plan !== false)
    writeFileSync(path.join(directory, "plan.json"), "{}");
  writeFileSync(path.join(directory, "result.json"), JSON.stringify(result));
}

async function runningRestart(s: Setup): Promise<OperatorProposalRecord> {
  const proposal = s.world.approve(s.propose(true));
  await s.world.service.drain();
  return s.world.h.core.operatorProposal(proposal.proposalId)!;
}

function ingest(s: Setup): readonly string[] {
  const { h } = s.world;
  return recoverRestartResults({
    stateDir: s.project.stateDir,
    runningRestarts: () =>
      h.core.listOperatorProposals({ states: ["running"] }),
    finishRun: (proposalId, report, durationMs) => {
      h.core.finishOperatorRun(ctx(h.core, h.owner), {
        proposalId,
        status: report.status,
        exitCode: report.exitCode,
        durationMs,
        outputTail: report.outputTail,
        truncated: false,
      });
    },
    notifyPm: (body) =>
      h.core.enqueueMessage(ctx(h.core, h.owner), {
        recipientAgentId: h.pm.agentId,
        body,
      }),
  });
}

test("a healthy result finishes the run, tells the PM and gives the operator the run message", async () => {
  const s = await setup();
  try {
    const running = await runningRestart(s);
    writeResult(s.project, running.proposalId, { outcome: "ok", pid: 123 });
    assert.deepEqual(ingest(s), [running.proposalId]);
    const done = s.world.h.core.operatorProposal(running.proposalId)!;
    assert.equal(done.state, "finished");
    assert.equal(done.run!.status, "ok");
    assert.ok(
      s
        .pmMessages()
        .some((body) => /finished: the controller restarted/.test(body)),
    );
    assert.ok(
      s.operatorMessages().some((body) => /finished exit 0/.test(body)),
    );
    assert.deepEqual(ingest(s), [], "a result is read once");
    assert.ok(
      existsSync(
        path.join(
          restartDirectory(s.project.stateDir, running.proposalId),
          "result.ingested.json",
        ),
      ),
    );
  } finally {
    await s.cleanup();
  }
});

test("a rolled-back result fails the run and warns the PM loudly when the ledger was restored", async () => {
  const s = await setup();
  try {
    const running = await runningRestart(s);
    writeResult(s.project, running.proposalId, {
      outcome: "rolled_back",
      reason: "the controller exited right after it started",
      ledgerRestored: true,
      failedLogTail: "boom",
    });
    ingest(s);
    const done = s.world.h.core.operatorProposal(running.proposalId)!;
    assert.equal(done.state, "failed");
    const notice = s.pmMessages().find((body) => /ROLLED BACK/.test(body))!;
    assert.match(notice, /LEDGER RESTORED/);
    assert.match(notice, /exited right after it started/);
    assert.match(done.run!.outputTail, /LEDGER RESTORED/);
  } finally {
    await s.cleanup();
  }
});

test("a down result names the manual recovery and the dependency change", async () => {
  const s = await setup();
  try {
    const running = await runningRestart(s);
    writeResult(s.project, running.proposalId, {
      outcome: "down",
      reason: "the known-good build did not start either",
      manualRecovery: "1. cp -a known-good/dist dist\n3. cstan start",
      depsChanged: ["package-lock.json"],
    });
    ingest(s);
    const done = s.world.h.core.operatorProposal(running.proposalId)!;
    assert.equal(done.state, "failed");
    assert.equal(done.run!.status, "error");
    const notice = s.pmMessages().find((body) => /FAILED/.test(body))!;
    assert.match(notice, /package-lock\.json/);
    assert.match(notice, /cstan start/);
  } finally {
    await s.cleanup();
  }
});

test("a stale running restart with no result and no live helper becomes abandoned; one with a plan and a live helper is left running", async () => {
  const s = await setup();
  try {
    const stale = await runningRestart(s);
    assert.equal(stale.state, "running");
    // Nothing alive behind pid 4194000: the plan is stale.
    await s.world.service.recover({
      skipRestartsWithPlan: skipRestartsWithLivePlan(s.project.stateDir),
    });
    assert.equal(
      s.world.h.core.operatorProposal(stale.proposalId)!.state,
      "abandoned",
    );

    const live = await runningRestart(s);
    writeFileSync(
      path.join(
        restartDirectory(s.project.stateDir, live.proposalId),
        "helper.pid",
      ),
      `${process.pid}\n`,
    );
    await s.world.service.recover({
      skipRestartsWithPlan: skipRestartsWithLivePlan(s.project.stateDir),
    });
    assert.equal(
      s.world.h.core.operatorProposal(live.proposalId)!.state,
      "running",
    );
  } finally {
    await s.cleanup();
  }
});

test("restart report and notice texts stay inside the message limits", () => {
  const huge = "x".repeat(200_000);
  for (const result of [
    { outcome: "ok" },
    {
      outcome: "rolled_back",
      reason: huge,
      failedLogTail: huge,
      ledgerRestored: true,
    },
    { outcome: "down", reason: huge, manualRecovery: huge, depsChanged: ["a"] },
  ] as RestartResult[]) {
    assert.ok(Buffer.byteLength(restartRunReport(result).outputTail) < 12288);
    assert.ok(Buffer.byteLength(restartNoticeToPm("r1", result)) < 16384);
  }
});

function snapshotProject(): FakeProject {
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  return project;
}

test("snapshot: known-good is created only after the settle time, when dist still equals the hash taken at start", async () => {
  const project = snapshotProject();
  try {
    const loaded = distHash(project.distDir);
    const schedule = scheduleKnownGoodSnapshot({
      stateDir: project.stateDir,
      projectRoot: project.root,
      distDir: project.distDir,
      loadedHash: loaded,
      settleMs: 300,
    });
    assert.equal(knownGoodBuild(project.stateDir).exists(), false);
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(
      knownGoodBuild(project.stateDir).exists(),
      false,
      "nothing before the settle time",
    );
    await waitFor(() => knownGoodBuild(project.stateDir).exists(), 5000);
    schedule.stop();
    const known = knownGoodBuild(project.stateDir);
    assert.equal(distHash(path.join(known.path, "dist")), loaded);
    const manifest = known.manifest()!;
    assert.equal(manifest.maxMigration, 3);
    assert.ok(Date.parse(manifest.createdAt) > 0);
    assert.ok(
      "package.json" in manifest.deps && "package-lock.json" in manifest.deps,
    );
    assert.ok("node_modules/.package-lock.json" in manifest.deps);
    assert.equal(typeof manifest.controllerVersion, "string");
  } finally {
    await project.cleanup();
  }
});

test("snapshot: a build run inside the window produces no snapshot and a restart then fails no_known_good", async () => {
  const project = snapshotProject();
  const events: string[] = [];
  try {
    const loaded = distHash(project.distDir);
    scheduleKnownGoodSnapshot({
      stateDir: project.stateDir,
      projectRoot: project.root,
      distDir: project.distDir,
      loadedHash: loaded,
      settleMs: 150,
      log: (event) => events.push(event),
    });
    writeFileSync(
      path.join(project.distDir, "src", "new-file.js"),
      "// a build ran\n",
    );
    await waitFor(() => events.length > 0, 5000);
    assert.deepEqual(events, ["known_good_skipped"]);
    assert.equal(knownGoodBuild(project.stateDir).exists(), false);
    const coordinator = createRestartCoordinator({
      stateDir: project.stateDir,
      projectRoot: project.root,
      distDir: project.distDir,
      helperSource: HELPER_SOURCE,
      node: process.execPath,
      argv: [],
      socketPath: project.socketPath,
      pidPath: project.pidPath,
      logPath: project.logPath,
      credentialFile: project.keyPath,
      healthTimeoutSeconds: 3,
      idleWaitSeconds: 1,
      busy: () => [],
      notifyPm: () => undefined,
      requestStop: () => assert.fail("must not stop"),
    });
    await assert.rejects(
      coordinator.run({
        proposalId: "r9",
        forceRestart: false,
      } as OperatorProposalRecord),
      (error: unknown) => (error as { code?: string }).code === "no_known_good",
    );
  } finally {
    await project.cleanup();
  }
});

test("snapshot: it is replaced atomically and leaves no staging or previous copy; a crashed replace is recovered", async () => {
  const project = snapshotProject();
  try {
    const known = knownGoodBuild(project.stateDir);
    known.snapshot(project.distDir, project.root);
    const first = known.manifest()!.createdAt;
    writeFileSync(
      path.join(project.distDir, "src", "second.js"),
      "// second\n",
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    known.snapshot(project.distDir, project.root);
    assert.notEqual(known.manifest()!.createdAt, first);
    assert.equal(
      distHash(path.join(known.path, "dist")),
      distHash(project.distDir),
    );
    assert.deepEqual(
      readdirSync(project.stateDir).filter((name) =>
        name.startsWith("known-good"),
      ),
      ["known-good"],
    );
    // A crash between the two renames leaves only the previous copy: it is put back.
    const previous = `${known.path}.previous`;
    rmSync(known.path, { recursive: true });
    mkdirSync(path.join(previous, "dist"), { recursive: true });
    writeFileSync(
      path.join(previous, "manifest.json"),
      JSON.stringify({
        createdAt: "x",
        maxMigration: 1,
        deps: {},
        controllerVersion: "1",
      }),
    );
    assert.equal(known.exists(), true);
    assert.equal(existsSync(previous), false);
  } finally {
    await project.cleanup();
  }
});

test("survival: the helper outlives the controller that started it, in its own process group, and completes the restart", async () => {
  const project = fakeProject();
  try {
    writeFakeBuild(project.distDir, "slow");
    knownGoodBuild(project.stateDir).snapshot(project.distDir, project.root);
    const cli = path.join(project.distDir, "src", "cli.js");
    const options = {
      stateDir: project.stateDir,
      projectRoot: project.root,
      distDir: project.distDir,
      helperSource: HELPER_SOURCE,
      node: process.execPath,
      argv: [cli, "daemon"],
      socketPath: project.socketPath,
      pidPath: project.pidPath,
      logPath: project.logPath,
      credentialFile: project.keyPath,
      healthTimeoutSeconds: 5,
      idleWaitSeconds: 1,
      timing: {
        handoffWaitMs: 5000,
        termWaitMs: 1000,
        killWaitMs: 2000,
        pollMs: 300,
      },
    };
    const driver = path.join(project.root, "driver.cjs");
    writeFileSync(
      driver,
      `require(${JSON.stringify(cli)});
       const options = ${JSON.stringify(options)};
       import(${JSON.stringify(path.resolve(import.meta.dirname, "..", "src", "restart.js"))}).then(async (restart) => {
         for (let i = 0; i < 100; i += 1) {
           if (require("node:fs").existsSync(options.socketPath)) break;
           await new Promise((resolve) => setTimeout(resolve, 50));
         }
         const coordinator = restart.createRestartCoordinator({
           ...options, busy: () => [], notifyPm: () => {},
           requestStop: () => process.kill(process.pid, "SIGTERM"),
         });
         await coordinator.run({ proposalId: "r1", forceRestart: true, kind: "restart" });
       }).catch((error) => { console.error(error); process.exit(9); });`,
    );
    const controller = spawn(process.execPath, [driver], {
      cwd: project.root,
      stdio: ["ignore", "ignore", "inherit"],
    });
    const controllerPid = controller.pid!;
    await new Promise<void>((resolve) =>
      controller.once("exit", () => resolve()),
    );
    assert.equal(alive(controllerPid), false, "the controller process is gone");
    const directory = restartDirectory(project.stateDir, "r1");
    const helperPid = Number.parseInt(
      readFileSync(path.join(directory, "helper.pid"), "utf8"),
      10,
    );
    assert.equal(alive(helperPid), true, "the helper is still running");
    const [pgid, ppid] = execFileSync("ps", [
      "-o",
      "pgid=,ppid=",
      "-p",
      String(helperPid),
    ])
      .toString()
      .trim()
      .split(/\s+/)
      .map(Number);
    assert.equal(pgid, helperPid, "the helper leads its own process group");
    assert.notEqual(
      ppid,
      controllerPid,
      "the helper is not a child of the controller",
    );
    await waitFor(() => existsSync(path.join(directory, "result.json")));
    const result = JSON.parse(
      readFileSync(path.join(directory, "result.json"), "utf8"),
    );
    assert.equal(result.outcome, "ok");
    const ping = await pingDaemon(project.socketPath, CREDENTIAL);
    assert.equal(ping.outcome, "running");
    assert.notEqual(ping.outcome === "running" ? ping.pid : 0, controllerPid);
  } finally {
    await stopFakeController(project);
    await project.cleanup();
  }
});

test("an unreadable operator key refuses the restart before any helper is started", async () => {
  const s = await setup();
  try {
    rmSync(s.project.keyPath);
    const result = await s.runRestart(true);
    assert.equal(result.state, "failed");
    assert.match(result.run!.outputTail, /^no_credential: /);
    assert.equal(s.spawned.length, 0);
    assert.equal(s.stops.length, 0);
    assert.ok(s.pmMessages().some((body) => /no_credential/.test(body)));
  } finally {
    await s.cleanup();
  }
});

test("a helper that cannot be spawned is a clean refusal: no plan left, run failed, PM told, no stop", async () => {
  const s = await setup({ realSpawnWithNode: "/nonexistent/node-binary" });
  try {
    const proposal = s.world.approve(s.propose(true));
    await s.world.service.drain();
    const result = s.world.h.core.operatorProposal(proposal.proposalId)!;
    assert.equal(result.state, "failed");
    assert.match(result.run!.outputTail, /^helper_start_failed: /);
    assert.equal(s.stops.length, 0);
    assert.equal(
      existsSync(
        path.join(
          restartDirectory(s.project.stateDir, proposal.proposalId),
          "plan.json",
        ),
      ),
      false,
    );
    assert.equal(
      s.pmMessages().some((m) => /helper_start_failed/.test(m)),
      true,
    );
    // let the async spawn error fire: it must not be an uncaught exception
    await new Promise((resolve) => setTimeout(resolve, 100));
  } finally {
    await s.cleanup();
  }
});

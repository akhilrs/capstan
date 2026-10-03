/**
 * Restart of the standalone binary: the helper (started as `<known-good binary> __restart-helper <plan>`)
 * waits for the old controller to go, starts the replaced binary with `daemon` and pings it; a replacement
 * that does not serve is renamed aside and the known-good binary is put back. Needs a built binary:
 * CSTAN_TEST_BINARY, or release/cstan-<version>-linux-x64 from `npm run build:binary`; skipped without one.
 */
import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { pingDaemon } from "../src/client.js";
import type { OperatorProposalRecord } from "../src/controller/types.js";
import {
  createRestartCoordinator,
  distHash,
  knownGoodBuild,
} from "../src/restart.js";
import type { RestartPlan, RestartResult } from "../src/restart-helper.js";

const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
);
const version = (
  JSON.parse(readFileSync(path.join(root, "package.json"), "utf8")) as {
    version: string;
  }
).version;
const binary =
  process.env.CSTAN_TEST_BINARY ??
  path.join(root, "release", `cstan-${version}-linux-x64`);
const runnable =
  existsSync(binary) && process.platform === "linux" && process.arch === "x64";

const sandboxes: string[] = [];
const pids: number[] = [];
after(() => {
  for (const pid of pids)
    try {
      process.kill(pid, "SIGKILL");
    } catch {
      // Already gone.
    }
  for (const dir of sandboxes) rmSync(dir, { recursive: true, force: true });
});

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function until(
  condition: () => boolean | Promise<boolean>,
  what: string,
): Promise<void> {
  for (let i = 0; i < 300; i += 1) {
    if (await condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${what}`);
}

interface Project {
  readonly dir: string;
  readonly installed: string;
  readonly stateDir: string;
  readonly env: NodeJS.ProcessEnv;
  readonly oldPid: number;
}

/** A temp project with a running daemon of the binary installed at <dir>/bin/cstan. */
async function runningProject(): Promise<Project> {
  const dir = mkdtempSync(path.join(tmpdir(), "capstan-sea-restart-"));
  sandboxes.push(dir);
  for (const sub of ["home", "bin", "repo", "tmp"])
    mkdirSync(path.join(dir, sub));
  const installed = path.join(dir, "bin", "cstan");
  copyFileSync(binary, installed);
  chmodSync(installed, 0o755);
  const repo = path.join(dir, "repo");
  const env: NodeJS.ProcessEnv = {
    HOME: path.join(dir, "home"),
    TMPDIR: path.join(dir, "tmp"),
    LANG: "C.UTF-8",
    PATH: "/usr/bin:/bin",
    CAPSTAN_LAUNCH: "off",
  };
  const git = (...args: string[]): void => {
    execFileSync(
      "git",
      ["-c", "user.name=t", "-c", "user.email=t@example.invalid", ...args],
      { cwd: repo, env, stdio: "ignore" },
    );
  };
  git("init", "-q", ".");
  git("commit", "-q", "--allow-empty", "-m", "init");
  execFileSync(installed, ["init"], { cwd: repo, env, stdio: "ignore" });
  spawnSyncIgnoringExit(installed, ["start"], repo, env);
  const stateDir = path.join(repo, ".capstan", "state");
  const oldPid = Number(
    readFileSync(path.join(stateDir, "daemon.pid"), "utf8"),
  );
  pids.push(oldPid);
  assert.ok(alive(oldPid));
  return { dir: repo, installed, stateDir, env, oldPid };
}

/** `cstan start` exits non-zero without Herdr after the daemon is up; only the daemon matters here. */
function spawnSyncIgnoringExit(
  command: string,
  args: string[],
  cwd: string,
  env: NodeJS.ProcessEnv,
): void {
  try {
    execFileSync(command, args, { cwd, env, stdio: "ignore", timeout: 60_000 });
  } catch {
    // Expected without Herdr.
  }
}

/** The known-good copy the daemon would have saved, and the plan the coordinator would write. */
function prepareRestart(project: Project): {
  planPath: string;
  restartDir: string;
  knownGood: string;
} {
  const knownGood = path.join(project.stateDir, "known-good");
  mkdirSync(knownGood, { recursive: true });
  copyFileSync(project.installed, path.join(knownGood, "cstan"));
  chmodSync(path.join(knownGood, "cstan"), 0o755);
  writeFileSync(
    path.join(knownGood, "manifest.json"),
    JSON.stringify({
      createdAt: new Date().toISOString(),
      controllerVersion: version,
      maxMigration: 9999,
      deps: {},
    }),
  );
  const restartDir = path.join(project.stateDir, "restart", "p1");
  mkdirSync(restartDir, { recursive: true });
  const capstanDir = path.dirname(project.stateDir);
  const plan: RestartPlan = {
    id: "p1",
    node: project.installed,
    argv: ["daemon"],
    cwd: project.dir,
    stateDir: project.stateDir,
    socketPath: path.join(project.stateDir, "control.sock"),
    pidPath: path.join(project.stateDir, "daemon.pid"),
    pid: project.oldPid,
    distPath: project.installed,
    binary: true,
    knownGoodPath: knownGood,
    ledgerPath: path.join(project.stateDir, "controller.sqlite"),
    logPath: path.join(capstanDir, "daemon.log"),
    healthTimeoutSeconds: 20,
    credentialFile: path.join(capstanDir, "operator.key"),
    depsChanged: [],
    timing: { handoffWaitMs: 1000, termWaitMs: 5000, pollMs: 200 },
  };
  const planPath = path.join(restartDir, "plan.json");
  writeFileSync(planPath, JSON.stringify(plan));
  return { planPath, restartDir, knownGood };
}

/** Replaces the installed binary the way install.sh does: a new file renamed over the old name. */
function replaceInstalled(project: Project, content: Buffer): void {
  const next = `${project.installed}.new`;
  writeFileSync(next, content, { mode: 0o755 });
  renameSync(next, project.installed);
}

async function runHelper(
  project: Project,
  prepared: { planPath: string; restartDir: string; knownGood: string },
): Promise<RestartResult> {
  // The coordinator runs the helper from the known-good binary, which outlives the replaced one.
  const child = spawn(
    path.join(prepared.knownGood, "cstan"),
    ["__restart-helper", prepared.planPath],
    { cwd: project.dir, env: project.env, detached: true, stdio: "ignore" },
  );
  child.unref();
  const resultPath = path.join(prepared.restartDir, "result.json");
  await until(() => existsSync(resultPath), "the restart result");
  return JSON.parse(readFileSync(resultPath, "utf8")) as RestartResult;
}

test(
  "a replaced binary is started by the helper, serves, and the old pid is gone",
  { skip: !runnable, timeout: 120_000 },
  async () => {
    const project = await runningProject();
    const prepared = prepareRestart(project);
    replaceInstalled(
      project,
      Buffer.concat([readFileSync(binary), Buffer.from("\n# replacement\n")]),
    );
    const replacementHash = distHash(project.installed);
    assert.notEqual(
      replacementHash,
      distHash(path.join(prepared.knownGood, "cstan")),
    );

    const result = await runHelper(project, prepared);
    assert.equal(result.outcome, "ok", JSON.stringify(result));
    assert.ok(result.pid !== undefined);
    pids.push(result.pid);
    assert.notEqual(result.pid, project.oldPid);
    await until(() => !alive(project.oldPid), "the old pid to exit");
    const ping = await pingDaemon(
      path.join(project.stateDir, "control.sock"),
      readFileSync(
        path.join(path.dirname(project.stateDir), "operator.key"),
        "utf8",
      ).trim(),
    );
    assert.equal(ping.outcome, "running");
    assert.equal(readlinkSync(`/proc/${result.pid}/exe`), project.installed);
    assert.equal(distHash(project.installed), replacementHash);
    assert.equal(
      readFileSync(path.join(project.stateDir, "daemon.pid"), "utf8").trim(),
      String(result.pid),
    );
  },
);

test(
  "a replacement that does not serve is renamed aside and the known-good binary is put back",
  { skip: !runnable, timeout: 120_000 },
  async () => {
    const project = await runningProject();
    const prepared = prepareRestart(project);
    const goodHash = distHash(project.installed);
    replaceInstalled(project, Buffer.from("#!/bin/sh\nexit 3\n"));

    const result = await runHelper(project, prepared);
    assert.equal(result.outcome, "rolled_back", JSON.stringify(result));
    assert.ok(result.pid !== undefined);
    pids.push(result.pid);
    assert.equal(distHash(project.installed), goodHash);
    assert.equal(readlinkSync(`/proc/${result.pid}/exe`), project.installed);
    assert.equal(
      readFileSync(`${project.installed}.failed-p1`, "utf8"),
      "#!/bin/sh\nexit 3\n",
    );
    await until(() => !alive(project.oldPid), "the old pid to exit");
  },
);

test(
  "the coordinator snapshots the binary, starts the helper from it and the replacement serves",
  { skip: !runnable, timeout: 120_000 },
  async () => {
    const project = await runningProject();
    const capstanDir = path.dirname(project.stateDir);
    const known = knownGoodBuild(project.stateDir, { binary: true });
    known.snapshot(project.installed, project.dir);
    assert.ok(existsSync(path.join(known.path, "cstan")));
    assert.deepEqual(known.manifest()?.deps, {});
    const coordinator = createRestartCoordinator({
      stateDir: project.stateDir,
      projectRoot: project.dir,
      distDir: project.installed,
      helperSource: "unused-under-the-binary",
      binary: true,
      node: project.installed,
      argv: ["daemon"],
      socketPath: path.join(project.stateDir, "control.sock"),
      pidPath: path.join(project.stateDir, "daemon.pid"),
      logPath: path.join(capstanDir, "daemon.log"),
      credentialFile: path.join(capstanDir, "operator.key"),
      healthTimeoutSeconds: 20,
      idleWaitSeconds: 1,
      busy: () => [],
      notifyPm: () => undefined,
      // The test process is not the daemon: stopping means signalling the real one.
      requestStop: () => process.kill(project.oldPid, "SIGTERM"),
      pid: project.oldPid,
      timing: { handoffWaitMs: 1000, termWaitMs: 5000, pollMs: 200 },
    });
    assert.deepEqual(coordinator.preflight(), { warning: null });
    replaceInstalled(
      project,
      Buffer.concat([readFileSync(binary), Buffer.from("\n# replacement\n")]),
    );
    const replacementHash = distHash(project.installed);
    await coordinator.run({
      proposalId: "p2",
      forceRestart: true,
    } as OperatorProposalRecord);
    const resultPath = path.join(
      project.stateDir,
      "restart",
      "p2",
      "result.json",
    );
    await until(() => existsSync(resultPath), "the restart result");
    const result = JSON.parse(
      readFileSync(resultPath, "utf8"),
    ) as RestartResult;
    assert.equal(result.outcome, "ok", JSON.stringify(result));
    assert.ok(result.pid !== undefined);
    pids.push(result.pid);
    await until(() => !alive(project.oldPid), "the old pid to exit");
    assert.equal(readlinkSync(`/proc/${result.pid}/exe`), project.installed);
    assert.equal(distHash(project.installed), replacementHash);
    assert.notEqual(distHash(path.join(known.path, "cstan")), replacementHash);
  },
);

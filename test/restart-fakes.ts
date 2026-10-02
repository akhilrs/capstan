import { spawn, type ChildProcess } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { callDaemon, pingDaemon } from "../src/client.js";
import type { RestartPlan } from "../src/restart-helper.js";

export type FakeMode =
  "good" | "slow" | "exit" | "silent" | "schema" | "migrate-exit" | "stubborn";

export const CREDENTIAL = "restart-test-credential-0123456789abcdef";

/**
 * A stand-in for `cstan daemon`: a CommonJS script that serves the control socket the way the daemon does
 * (one JSON frame per line, `ping` and `shutdown`), with failure modes the restart tests need.
 */
export function fakeCliSource(
  mode: FakeMode,
  options: { maxMigration?: number; startsFile?: string } = {},
): string {
  return `
const net = require("node:net");
const fs = require("node:fs");
const path = require("node:path");
const MODE = ${JSON.stringify(mode)};
const MAX = ${options.maxMigration ?? 0};
const STARTS = ${JSON.stringify(options.startsFile ?? null)};
const state = path.join(process.cwd(), ".capstan", "state");
const socketPath = path.join(state, "control.sock");
const pidPath = path.join(state, "daemon.pid");
const ledger = path.join(state, "controller.sqlite");
const credential = fs.readFileSync(path.join(process.cwd(), ".capstan", "operator.key"), "utf8").trim();
if (STARTS) fs.appendFileSync(STARTS, MODE + "\\n");
function exitNow(code) { process.exit(code); }
if (MODE === "exit") { console.log("boom: the fake controller cannot start"); exitNow(3); }
if (MODE === "migrate-exit") {
  const { DatabaseSync } = require("node:sqlite");
  const db = new DatabaseSync(ledger);
  db.prepare("INSERT INTO schema_migrations(version) VALUES (?)").run(MAX + 1);
  db.prepare("INSERT INTO messages(body) VALUES ('new-row')").run();
  db.close();
  console.log("migrated the ledger to a newer schema, then failed");
  exitNow(1);
}
if (MODE === "schema") {
  const { DatabaseSync } = require("node:sqlite");
  const db = new DatabaseSync(ledger, { readOnly: true });
  const version = db.prepare("SELECT MAX(version) AS v FROM schema_migrations").get().v;
  db.close();
  if (version > MAX) { console.log("database schema version " + version + " is newer than this controller"); exitNow(1); }
}
if (MODE === "silent") { setInterval(() => {}, 1000); }
else {
  const shutdown = () => {
    server.close();
    fs.rmSync(socketPath, { force: true });
    fs.rmSync(pidPath, { force: true });
    process.exit(0);
  };
  if (MODE !== "stubborn") process.on("SIGTERM", shutdown);
  else process.on("SIGTERM", () => {});
  const server = net.createServer((connection) => {
    let data = "";
    connection.on("data", (chunk) => {
      data += chunk;
      const newline = data.indexOf("\\n");
      if (newline < 0) return;
      let frame;
      try { frame = JSON.parse(data.slice(0, newline)); } catch { connection.end(); return; }
      if (frame.credential !== credential) { connection.end(JSON.stringify({ ok: false, code: "unauthenticated", message: "no" }) + "\\n"); return; }
      if (frame.command === "ping") connection.end(JSON.stringify({ ok: true, result: { pid: process.pid } }) + "\\n");
      else if (frame.command === "shutdown") { connection.end(JSON.stringify({ ok: true, result: {} }) + "\\n"); setImmediate(shutdown); }
      else connection.end(JSON.stringify({ ok: false, code: "unknown", message: "no" }) + "\\n");
    });
    connection.on("error", () => {});
  });
  const listen = () => {
    fs.rmSync(socketPath, { force: true });
    server.listen(socketPath, () => fs.writeFileSync(pidPath, process.pid + "\\n"));
  };
  if (MODE === "slow") setTimeout(listen, 800); else listen();
}
`;
}

export interface FakeProject {
  readonly root: string;
  readonly stateDir: string;
  readonly distDir: string;
  readonly socketPath: string;
  readonly pidPath: string;
  readonly logPath: string;
  readonly keyPath: string;
  readonly startsFile: string;
  readonly ledgerPath: string;
  readonly knownGoodPath: string;
  cleanup(): Promise<void>;
}

/** A temporary project root with `.capstan/operator.key`, a state directory and room for dist. */
export function fakeProject(): FakeProject {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-restart-"));
  const capstan = path.join(root, ".capstan");
  const stateDir = path.join(capstan, "state");
  mkdirSync(stateDir, { recursive: true, mode: 0o700 });
  const keyPath = path.join(capstan, "operator.key");
  writeFileSync(keyPath, `${CREDENTIAL}\n`, { mode: 0o600 });
  chmodSync(keyPath, 0o600);
  const project: FakeProject = {
    root,
    stateDir,
    distDir: path.join(root, "dist"),
    socketPath: path.join(stateDir, "control.sock"),
    pidPath: path.join(stateDir, "daemon.pid"),
    logPath: path.join(capstan, "daemon.log"),
    keyPath,
    startsFile: path.join(root, "starts.txt"),
    ledgerPath: path.join(stateDir, "controller.sqlite"),
    knownGoodPath: path.join(stateDir, "known-good"),
    async cleanup() {
      for (const pid of controllerPids(this)) {
        try {
          process.kill(pid, "SIGKILL");
        } catch {
          // Already gone.
        }
      }
      rmSync(root, { recursive: true, force: true });
    },
  };
  return project;
}

function controllerPids(project: FakeProject): number[] {
  try {
    return [Number.parseInt(readFileSync(project.pidPath, "utf8"), 10)].filter(
      Number.isInteger,
    );
  } catch {
    return [];
  }
}

/** Writes a fake build to `directory` (dist or a known-good copy): src/cli.js and a migrations folder. */
export function writeFakeBuild(
  directory: string,
  mode: FakeMode,
  options: {
    maxMigration?: number;
    startsFile?: string;
    migrations?: number;
  } = {},
): void {
  mkdirSync(path.join(directory, "src"), { recursive: true });
  mkdirSync(path.join(directory, "migrations"), { recursive: true });
  writeFileSync(
    path.join(directory, "src", "cli.js"),
    fakeCliSource(mode, options),
  );
  for (let index = 1; index <= (options.migrations ?? 3); index += 1)
    writeFileSync(
      path.join(
        directory,
        "migrations",
        `${String(index).padStart(4, "0")}_x.sql`,
      ),
      `-- ${index}\n`,
    );
}

/** Starts the fake controller of `project` as its own process; resolves when it answers ping. */
export async function startFakeController(
  project: FakeProject,
): Promise<ChildProcess & { pid: number }> {
  const child = spawn(
    process.execPath,
    [path.join(project.distDir, "src", "cli.js"), "daemon"],
    {
      cwd: project.root,
      detached: true,
      stdio: ["ignore", "ignore", "ignore"],
    },
  );
  child.unref();
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (
      (await pingDaemon(project.socketPath, CREDENTIAL)).outcome === "running"
    )
      return child as ChildProcess & { pid: number };
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("the fake controller did not answer ping");
}

export async function stopFakeController(project: FakeProject): Promise<void> {
  try {
    await callDaemon(project.socketPath, CREDENTIAL, "shutdown");
  } catch {
    // Not running.
  }
}

export function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export async function waitFor(
  condition: () => boolean | Promise<boolean>,
  limitMs = 15_000,
): Promise<void> {
  const deadline = Date.now() + limitMs;
  while (!(await condition())) {
    if (Date.now() > deadline) throw new Error("timed out waiting");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}

/** A plan with short timings for `project`; `overrides` replace any field. */
export function planFor(
  project: FakeProject,
  overrides: Partial<RestartPlan> = {},
): RestartPlan {
  return {
    id: "r1",
    node: process.execPath,
    argv: [path.join(project.distDir, "src", "cli.js"), "daemon"],
    cwd: project.root,
    stateDir: project.stateDir,
    socketPath: project.socketPath,
    pidPath: project.pidPath,
    pid: 2_000_000_000,
    distPath: project.distDir,
    knownGoodPath: project.knownGoodPath,
    ledgerPath: project.ledgerPath,
    logPath: project.logPath,
    healthTimeoutSeconds: 3,
    credentialFile: project.keyPath,
    depsChanged: [],
    timing: {
      handoffWaitMs: 500,
      termWaitMs: 400,
      killWaitMs: 2000,
      pollMs: 100,
    },
    ...overrides,
  };
}

export function writePlan(project: FakeProject, plan: RestartPlan): string {
  const directory = path.join(project.stateDir, "restart", plan.id);
  mkdirSync(directory, { recursive: true });
  const planPath = path.join(directory, "plan.json");
  writeFileSync(planPath, JSON.stringify(plan));
  return planPath;
}

export function moduleUrl(relativeToDist: string): string {
  return pathToFileURL(path.resolve(import.meta.dirname, "..", relativeToDist))
    .href;
}

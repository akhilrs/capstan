/**
 * The restart helper. The controller copies this file out of dist to `<state>/restart/<id>/helper.mjs`
 * and starts it detached, so it outlives the controller and the dist directory it replaces. It uses Node
 * builtins only and imports nothing from the rest of the project.
 *
 * Order: wait for the old controller to be gone, back the ledger up, start the new build, ping it
 * directly over the control socket. A new build that does not answer is stopped, dist is renamed to
 * `dist.failed-<id>` and the known-good copy takes its place; the ledger backup is restored first when
 * the stored schema is newer than the known-good build knows. At most two start attempts per build, and
 * nothing the helper did not create is deleted.
 */
import { spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { pathToFileURL } from "node:url";

export interface RestartTiming {
  /** How long to wait for the old controller to be gone before it is signalled. */
  readonly handoffWaitMs: number;
  /** How long the old controller gets after SIGTERM before SIGKILL. */
  readonly termWaitMs: number;
  /** How long a process group gets to end after a signal. */
  readonly killWaitMs: number;
  readonly pollMs: number;
}

export const DEFAULT_RESTART_TIMING: RestartTiming = {
  handoffWaitMs: 30_000,
  termWaitMs: 10_000,
  killWaitMs: 5_000,
  pollMs: 1_000,
};

export interface RestartPlan {
  readonly id: string;
  /** The node binary and the arguments that start the controller. */
  readonly node: string;
  readonly argv: readonly string[];
  readonly cwd: string;
  readonly stateDir: string;
  readonly socketPath: string;
  readonly pidPath: string;
  /** The pid of the controller that asked for the restart. */
  readonly pid: number;
  readonly distPath: string;
  readonly knownGoodPath: string;
  readonly ledgerPath: string;
  readonly logPath: string;
  readonly healthTimeoutSeconds: number;
  readonly credentialFile: string;
  /** Dependency files that differ from the known-good manifest at the time of the proposal. */
  readonly depsChanged: readonly string[];
  readonly timing?: Partial<RestartTiming>;
}

export type RestartOutcome = "ok" | "rolled_back" | "down";

export interface RestartResult {
  readonly outcome: RestartOutcome;
  readonly reason?: string;
  readonly failedLogTail?: string;
  readonly ledgerRestored?: boolean;
  readonly manualRecovery?: string;
  readonly depsChanged?: readonly string[];
  readonly pid?: number;
}

export const EXIT_OK = 0;
export const EXIT_ROLLED_BACK = 1;
export const EXIT_DOWN = 2;
const START_ATTEMPTS = 2;
const LOG_TAIL_CHARS = 600;
const PING_TIMEOUT_MS = 3_000;
const SETTLE_RECHECK_MS = 2_000;

/** The frame `callDaemon` in src/client.ts sends; a contract test pins the two together. */
export function pingFrame(credential: string): string {
  return `${JSON.stringify({ v: 1, credential, command: "ping", args: [] })}\n`;
}

export type PingProbe =
  | { readonly kind: "ok"; readonly pid: number | null }
  | { readonly kind: "refused" }
  | { readonly kind: "failed" };

/** One direct ping over the control socket. `refused` means nothing listens (no socket file or a dead listener). */
export function directPing(
  socketPath: string,
  credential: string,
  timeoutMs = PING_TIMEOUT_MS,
): Promise<PingProbe> {
  return new Promise((resolve) => {
    const socket = net.createConnection(socketPath);
    let bytes = Buffer.alloc(0);
    let settled = false;
    const settle = (probe: PingProbe): void => {
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(probe);
    };
    socket.once("connect", () => socket.write(pingFrame(credential)));
    socket.on("data", (chunk: Buffer) => {
      bytes = Buffer.concat([bytes, chunk]);
      const newline = bytes.indexOf(10);
      if (newline < 0) {
        if (bytes.length > 1024 * 1024) settle({ kind: "failed" });
        return;
      }
      try {
        const body = JSON.parse(
          bytes.subarray(0, newline).toString("utf8"),
        ) as { ok?: unknown; result?: { pid?: unknown } };
        if (body.ok !== true) return settle({ kind: "failed" });
        const pid = body.result?.pid;
        settle({ kind: "ok", pid: typeof pid === "number" ? pid : null });
      } catch {
        settle({ kind: "failed" });
      }
    });
    socket.once("error", (error: NodeJS.ErrnoException) =>
      settle(
        error.code === "ENOENT" || error.code === "ECONNREFUSED"
          ? { kind: "refused" }
          : { kind: "failed" },
      ),
    );
    socket.once("close", () => settle({ kind: "failed" }));
    socket.setTimeout(timeoutMs, () => settle({ kind: "failed" }));
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

function groupAlive(pgid: number): boolean {
  try {
    process.kill(-pgid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

async function waitUntil(
  condition: () => boolean | Promise<boolean>,
  limitMs: number,
  pollMs: number,
): Promise<boolean> {
  const deadline = Date.now() + limitMs;
  for (;;) {
    if (await condition()) return true;
    if (Date.now() >= deadline) return false;
    await sleep(Math.min(pollMs, Math.max(1, deadline - Date.now())));
  }
}

function readPidFile(pidPath: string): number | null {
  try {
    const value = Number.parseInt(fs.readFileSync(pidPath, "utf8").trim(), 10);
    return Number.isInteger(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}

function pidFileExists(pidPath: string): boolean {
  return fs.existsSync(pidPath);
}

/** A pid file that is gone, or names a dead process, no longer blocks a start. */
function pidFileFree(pidPath: string): boolean {
  if (!pidFileExists(pidPath)) return true;
  const pid = readPidFile(pidPath);
  return pid === null || !processAlive(pid);
}

function writeFileAtomic(target: string, text: string): void {
  const temporary = `${target}.tmp-${process.pid}`;
  fs.writeFileSync(temporary, text, { mode: 0o600 });
  fs.renameSync(temporary, target);
}

function sanitize(text: string): string {
  return text.replace(/[\p{Cc}\p{Cf}]/gu, (ch) => (ch === "\n" ? "\n" : " "));
}

function logTailFrom(logPath: string, fromOffset: number): string {
  try {
    const size = fs.statSync(logPath).size;
    const start = Math.max(fromOffset, size - LOG_TAIL_CHARS * 2);
    const fd = fs.openSync(logPath, "r");
    try {
      const buffer = Buffer.alloc(Math.max(0, size - start));
      fs.readSync(fd, buffer, 0, buffer.length, start);
      return sanitize(buffer.toString("utf8")).trimEnd().slice(-LOG_TAIL_CHARS);
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return "";
  }
}

const LEDGER_SUFFIXES = ["", "-wal", "-shm"] as const;

/** Copies the database and its -wal and -shm files; the old controller has exited, so they are stable. */
export function backupLedger(ledgerPath: string, backupDir: string): void {
  fs.mkdirSync(backupDir, { recursive: true, mode: 0o700 });
  for (const suffix of LEDGER_SUFFIXES) {
    const source = `${ledgerPath}${suffix}`;
    if (fs.existsSync(source))
      fs.copyFileSync(source, path.join(backupDir, path.basename(source)));
  }
}

/** Puts the backup back, replacing the live files, so no -wal of a newer schema stays beside an older database. */
export function restoreLedger(ledgerPath: string, backupDir: string): void {
  for (const suffix of LEDGER_SUFFIXES) {
    const target = `${ledgerPath}${suffix}`;
    const backup = path.join(backupDir, path.basename(target));
    if (fs.existsSync(backup)) {
      const temporary = `${target}.restoring`;
      fs.copyFileSync(backup, temporary);
      fs.renameSync(temporary, target);
    } else fs.rmSync(target, { force: true });
  }
}

/** The highest migration the stored ledger records, or null when node:sqlite or the table is unavailable. */
export async function storedSchemaVersion(
  ledgerPath: string,
): Promise<number | null> {
  try {
    const sqlite =
      (await import("node:sqlite")) as typeof import("node:sqlite");
    const database = new sqlite.DatabaseSync(ledgerPath, { readOnly: true });
    try {
      const row = database
        .prepare("SELECT MAX(version) AS version FROM schema_migrations")
        .get() as { version: number | null } | undefined;
      return typeof row?.version === "number" ? row.version : 0;
    } finally {
      database.close();
    }
  } catch {
    return null;
  }
}

interface Manifest {
  readonly maxMigration: number;
}

function readManifest(knownGoodPath: string): Manifest | null {
  try {
    const parsed = JSON.parse(
      fs.readFileSync(path.join(knownGoodPath, "manifest.json"), "utf8"),
    ) as { maxMigration?: unknown };
    return typeof parsed.maxMigration === "number"
      ? { maxMigration: parsed.maxMigration }
      : null;
  } catch {
    return null;
  }
}

class Helper {
  readonly timing: RestartTiming;
  readonly credential: string;
  readonly restartDir: string;
  readonly backupDir: string;
  readonly failedDist: string;
  #exit: { code: number | null; signal: string | null } | null = null;

  constructor(
    readonly plan: RestartPlan,
    restartDir: string,
    private readonly log: (message: string) => void,
  ) {
    this.timing = { ...DEFAULT_RESTART_TIMING, ...plan.timing };
    this.credential = fs.readFileSync(plan.credentialFile, "utf8").trim();
    this.restartDir = restartDir;
    this.backupDir = path.join(restartDir, "ledger.bak");
    this.failedDist = path.join(
      path.dirname(plan.distPath),
      `dist.failed-${plan.id}`,
    );
  }

  async #socketRefuses(): Promise<boolean> {
    return (
      (await directPing(this.plan.socketPath, this.credential)).kind ===
      "refused"
    );
  }

  /** The old controller is gone when its pid, its socket and its pid file are. Returns a reason on failure. */
  async handoff(): Promise<string | null> {
    const { plan, timing } = this;
    const settled = async (): Promise<boolean> =>
      !processAlive(plan.pid) &&
      (await this.#socketRefuses()) &&
      pidFileFree(plan.pidPath);
    if (await waitUntil(settled, timing.handoffWaitMs, timing.pollMs))
      return this.#clearOwnPidFile();
    if (processAlive(plan.pid)) {
      this.log(`old controller ${plan.pid} still alive: SIGTERM`);
      process.kill(plan.pid, "SIGTERM");
      if (
        !(await waitUntil(
          () => !processAlive(plan.pid),
          timing.termWaitMs,
          Math.min(timing.pollMs, 100),
        ))
      ) {
        this.log(`old controller ${plan.pid} ignored SIGTERM: SIGKILL`);
        try {
          process.kill(plan.pid, "SIGKILL");
        } catch {
          // Gone between the check and the signal.
        }
        await waitUntil(
          () => !processAlive(plan.pid),
          timing.killWaitMs,
          Math.min(timing.pollMs, 100),
        );
      }
    }
    if (processAlive(plan.pid))
      return `the old controller (pid ${plan.pid}) did not exit`;
    this.#clearOwnPidFile();
    if (!(await waitUntil(settled, SETTLE_RECHECK_MS, 100)))
      return "another process still holds the control socket or the pid file";
    return null;
  }

  /** Removes the pid file only when it names the old, dead pid; another live pid's file is never touched. */
  #clearOwnPidFile(): null {
    const { plan } = this;
    if (readPidFile(plan.pidPath) === plan.pid && !processAlive(plan.pid))
      fs.rmSync(plan.pidPath, { force: true });
    return null;
  }

  #start(): { child: ChildProcess; logOffset: number } {
    const { plan } = this;
    let logOffset = 0;
    try {
      logOffset = fs.statSync(plan.logPath).size;
    } catch {
      // The log does not exist yet; the start creates it.
    }
    const fd = fs.openSync(plan.logPath, "a", 0o600);
    let child: ChildProcess;
    try {
      child = spawn(plan.node, [...plan.argv], {
        cwd: plan.cwd,
        detached: true,
        stdio: ["ignore", fd, fd],
        env: scrubbed(process.env),
      });
    } finally {
      fs.closeSync(fd);
    }
    this.#exit = null;
    child.once("exit", (code, signal) => {
      this.#exit = { code, signal };
    });
    child.once("error", (error) => {
      this.#exit = { code: null, signal: `spawn failed: ${error.message}` };
    });
    child.unref();
    return { child, logOffset };
  }

  /** Polls until the controller answers ping, exits, or the timeout passes. Returns a reason when unhealthy. */
  async #health(child: ChildProcess): Promise<{ pid: number } | string> {
    const { plan, timing } = this;
    const deadline = Date.now() + plan.healthTimeoutSeconds * 1000;
    for (;;) {
      if (this.#exit !== null)
        return `the controller exited right after it started (${this.#exit.signal ?? `exit code ${this.#exit.code}`})`;
      const probe = await directPing(plan.socketPath, this.credential);
      if (probe.kind === "ok") {
        if (
          probe.pid !== null &&
          child.pid !== undefined &&
          probe.pid !== child.pid
        )
          return `a different process (pid ${probe.pid}) answered ping instead of the new controller (pid ${child.pid})`;
        return { pid: child.pid ?? probe.pid ?? 0 };
      }
      if (Date.now() >= deadline)
        return `the controller did not answer ping within ${plan.healthTimeoutSeconds} seconds`;
      await sleep(Math.min(timing.pollMs, Math.max(1, deadline - Date.now())));
    }
  }

  /** Starts the controller and waits for it to be healthy. A failed attempt stops the process group it started. */
  async startAndCheck(): Promise<
    { pid: number } | { reason: string; logTail: string }
  > {
    const started = this.#start();
    const outcome = await this.#health(started.child);
    if (typeof outcome !== "string") return outcome;
    const logTail = logTailFrom(this.plan.logPath, started.logOffset);
    await this.#stopGroup(started.child);
    return { reason: outcome, logTail };
  }

  async #stopGroup(child: ChildProcess): Promise<void> {
    const pgid = child.pid;
    if (pgid === undefined) return;
    const { timing } = this;
    for (const [signal, limit] of [
      ["SIGTERM", timing.termWaitMs],
      ["SIGKILL", timing.killWaitMs],
    ] as const) {
      if (!groupAlive(pgid)) return;
      try {
        process.kill(-pgid, signal);
      } catch {
        // The group ended between the check and the signal.
      }
      if (
        await waitUntil(
          () => !groupAlive(pgid),
          limit,
          Math.min(timing.pollMs, 100),
        )
      )
        return;
    }
  }

  /** dist -> dist.failed-<id>, then a complete copy of the known-good build renamed into place. */
  restoreKnownGood(): string | null {
    const { plan } = this;
    const source = path.join(plan.knownGoodPath, "dist");
    if (!fs.existsSync(source))
      return `the known-good build ${source} is missing`;
    const staging = path.join(
      path.dirname(plan.distPath),
      `dist.restoring-${plan.id}`,
    );
    let renamedFailed = false;
    try {
      if (fs.existsSync(plan.distPath)) {
        fs.renameSync(plan.distPath, this.failedDist);
        renamedFailed = true;
      }
      fs.rmSync(staging, { recursive: true, force: true });
      fs.cpSync(source, staging, {
        recursive: true,
        preserveTimestamps: true,
        verbatimSymlinks: true,
      });
      fs.renameSync(staging, plan.distPath);
      return null;
    } catch (error) {
      fs.rmSync(staging, { recursive: true, force: true });
      if (renamedFailed && !fs.existsSync(plan.distPath))
        try {
          fs.renameSync(this.failedDist, plan.distPath);
        } catch {
          // Kept as dist.failed-<id>; the manual recovery text names it.
        }
      return `restoring the known-good build failed: ${String(error)}`;
    }
  }

  manualRecovery(): string {
    const { plan } = this;
    return [
      `Run these from the project root (${plan.cwd}):`,
      `1. Replace dist with the saved build: rm -rf dist && cp -a ${path.join(plan.knownGoodPath, "dist")} dist (the failed build is kept as ${this.failedDist}).`,
      `2. If the controller still refuses the ledger, restore the backup in ${this.backupDir} over ${plan.ledgerPath} (with its -wal and -shm files).`,
      "3. Start the controller with: cstan start",
    ].join("\n");
  }
}

function scrubbed(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const copy = { ...env };
  delete copy.CAPSTAN_TOKEN;
  delete copy.CAPSTAN_SOCKET;
  return copy;
}

function writeResult(restartDir: string, result: RestartResult): number {
  writeFileAtomic(
    path.join(restartDir, "result.json"),
    `${JSON.stringify(result)}\n`,
  );
  return result.outcome === "ok"
    ? EXIT_OK
    : result.outcome === "rolled_back"
      ? EXIT_ROLLED_BACK
      : EXIT_DOWN;
}

/** Runs one restart from its plan file and returns the exit code: 0 ok, 1 rolled back, 2 down. */
export async function runHelper(
  planPath: string,
  log: (message: string) => void = (message) =>
    process.stdout.write(`${new Date().toISOString()} ${message}\n`),
): Promise<number> {
  const restartDir = path.dirname(planPath);
  let plan: RestartPlan;
  let helper: Helper;
  try {
    plan = JSON.parse(fs.readFileSync(planPath, "utf8")) as RestartPlan;
    helper = new Helper(plan, restartDir, log);
  } catch (error) {
    // The controller is already stopping: say so in a result instead of leaving the run open.
    return writeResult(restartDir, {
      outcome: "down",
      reason: `the restart helper could not read its plan or the operator key (${String(error)}); the controller was stopped and is NOT running`,
      manualRecovery: [
        "Run these from the project root:",
        "1. Check dist (the restart changed nothing before this failure) and, if needed, restore it from .capstan/state/known-good/dist.",
        "2. Start the controller with: cstan start",
      ].join("\n"),
    });
  }
  try {
    return await runSteps(plan, helper, restartDir, log);
  } catch (error) {
    return writeResult(restartDir, {
      outcome: "down",
      reason: `the restart helper failed unexpectedly (${String(error)}); the controller may NOT be running`,
      manualRecovery: helper.manualRecovery(),
      ...((plan.depsChanged ?? []).length > 0
        ? { depsChanged: plan.depsChanged }
        : {}),
    });
  }
}

async function runSteps(
  plan: RestartPlan,
  helper: Helper,
  restartDir: string,
  log: (message: string) => void,
): Promise<number> {
  const down = (reason: string, extra: Partial<RestartResult> = {}): number =>
    writeResult(restartDir, {
      outcome: "down",
      reason,
      manualRecovery: helper.manualRecovery(),
      ...(plan.depsChanged.length > 0 ? { depsChanged: plan.depsChanged } : {}),
      ...extra,
    });

  const refused = await helper.handoff();
  if (refused !== null) {
    log(`handoff failed: ${refused}`);
    // The handoff may already have signalled the old controller: only a controller that still answers
    // ping counts as "nothing changed".
    const serving = await directPing(plan.socketPath, helper.credential);
    if (serving.kind === "ok" && serving.pid === plan.pid)
      return writeResult(restartDir, {
        outcome: "rolled_back",
        reason: `the restart did not start the new build and the old controller (pid ${plan.pid}) still answers ping: ${refused}`,
      });
    if (serving.kind === "ok")
      return down(
        `the restart stopped before the new build started (${refused}), but a controller other than the old one (pid ${plan.pid}) answers ping${serving.pid === null ? "" : ` (pid ${serving.pid})`}: check which build it runs`,
      );
    return down(
      `the restart stopped before the new build started (${refused}) and no controller answers ping: the controller is NOT running`,
    );
  }

  let failure: { reason: string; logTail: string } | undefined;
  try {
    backupLedger(plan.ledgerPath, helper.backupDir);
  } catch (error) {
    failure = {
      reason: `the ledger could not be backed up (${String(error)}); the new build was not started`,
      logTail: "",
    };
  }
  if (failure === undefined) {
    log("starting the new build");
    const first = await helper.startAndCheck();
    if ("pid" in first) {
      log(`the new controller answers ping (pid ${first.pid})`);
      return writeResult(restartDir, { outcome: "ok", pid: first.pid });
    }
    failure = first;
  }
  log(`new build failed: ${failure.reason}`);

  const restored = helper.restoreKnownGood();
  if (restored !== null) return down(restored);

  let ledgerRestored = false;
  const manifest = readManifest(plan.knownGoodPath);
  if (manifest !== null) {
    const stored = await storedSchemaVersion(plan.ledgerPath);
    if (stored !== null && stored > manifest.maxMigration) {
      try {
        restoreLedger(plan.ledgerPath, helper.backupDir);
        ledgerRestored = true;
        log(
          `ledger schema ${stored} is newer than the known-good build (${manifest.maxMigration}): ledger restored from backup`,
        );
      } catch (error) {
        return down(
          `the ledger schema ${stored} is newer than the known-good build and restoring the backup failed: ${String(error)}`,
        );
      }
    }
  }

  let last = "no start attempt was made";
  for (let attempt = 1; attempt <= START_ATTEMPTS; attempt += 1) {
    log(`starting the known-good build (attempt ${attempt})`);
    const restart = await helper.startAndCheck();
    if ("pid" in restart)
      return writeResult(restartDir, {
        outcome: "rolled_back",
        reason: failure.reason,
        failedLogTail: failure.logTail,
        ledgerRestored,
        ...(plan.depsChanged.length > 0
          ? { depsChanged: plan.depsChanged }
          : {}),
        pid: restart.pid,
      });
    last = restart.reason;
  }
  const dependencyNote =
    plan.depsChanged.length > 0
      ? ` Dependencies changed since the known-good build (${plan.depsChanged.join(", ")}), which can stop the old build from starting.`
      : "";
  return down(
    `the new build failed (${failure.reason}) and the known-good build did not start either (${last}).${dependencyNote}`,
    { failedLogTail: failure.logTail, ledgerRestored },
  );
}

const entry = process.argv[1];
if (entry !== undefined && import.meta.url === pathToFileURL(entry).href) {
  const planPath = process.argv[2];
  if (planPath === undefined) {
    process.stderr.write("usage: helper.mjs <plan.json>\n");
    process.exit(64);
  }
  runHelper(planPath)
    .then((code) => process.exit(code))
    .catch((error: unknown) => {
      process.stderr.write(`restart helper failed: ${String(error)}\n`);
      process.exit(EXIT_DOWN);
    });
}

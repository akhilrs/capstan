/**
 * Controller side of an Operator restart: the known-good build, the busy check, the coordinator that
 * hands the restart to a detached helper, and the ingestion of the helper's result by the next
 * controller. The helper itself is src/restart-helper.ts.
 */
import { spawn as nodeSpawn, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { OperatorError } from "./operator.js";
import type {
  OperatorProposalRecord,
  OperatorRunStatus,
} from "./controller/types.js";
import type {
  RestartOutcome,
  RestartPlan,
  RestartResult,
  RestartTiming,
} from "./restart-helper.js";

export const KNOWN_GOOD_SETTLE_MS = 60_000;
export const RESTART_DIR_NAME = "restart";
export const KNOWN_GOOD_DIR_NAME = "known-good";
const DEPENDENCY_FILES = [
  "package.json",
  "package-lock.json",
  "node_modules/.package-lock.json",
] as const;
const IDLE_POLL_MS = 1000;
const NOTICE_TAIL_CHARS = 1500;
const MAX_REPORT_CHARS = 6000;

/** A refusal with a stable code; the service shows `code: message` to the Operator. */
export class RestartRefusal extends OperatorError {}

/** sha256 over the sorted file tree of a directory: relative path, then content (or link target). */
export function distHash(directory: string): string {
  const hash = createHash("sha256");
  const walk = (current: string, relative: string): void => {
    const entries = fs
      .readdirSync(current, { withFileTypes: true })
      .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
    for (const entry of entries) {
      const entryRelative =
        relative === "" ? entry.name : `${relative}/${entry.name}`;
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) {
        hash.update(`d:${entryRelative}\0`);
        walk(full, entryRelative);
      } else if (entry.isSymbolicLink()) {
        hash.update(`l:${entryRelative}\0${fs.readlinkSync(full)}\0`);
      } else if (entry.isFile()) {
        hash.update(`f:${entryRelative}\0`);
        hash.update(fs.readFileSync(full));
        hash.update("\0");
      }
    }
  };
  walk(directory, "");
  return hash.digest("hex");
}

function fileSha(file: string): string | null {
  try {
    return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
  } catch {
    return null;
  }
}

function dependencyHashes(root: string): Record<string, string | null> {
  const out: Record<string, string | null> = {};
  for (const name of DEPENDENCY_FILES)
    out[name] = fileSha(path.join(root, name));
  return out;
}

/** The highest migration number among the SQL files a build ships. */
export function maxMigrationOf(distDirectory: string): number {
  let highest = 0;
  try {
    for (const name of fs.readdirSync(path.join(distDirectory, "migrations"))) {
      const match = /^(\d+)_.*\.sql$/.exec(name);
      if (match !== null) highest = Math.max(highest, Number(match[1]));
    }
  } catch {
    // A build without migrations has none.
  }
  return highest;
}

export interface KnownGoodManifest {
  readonly createdAt: string;
  readonly controllerVersion: string;
  readonly maxMigration: number;
  readonly deps: Readonly<Record<string, string | null>>;
}

export interface KnownGoodBuild {
  readonly path: string;
  /** Whether a complete snapshot exists, with a readable manifest. */
  exists(): boolean;
  manifest(): KnownGoodManifest | undefined;
  /** Dependency files whose hash differs from the manifest. Empty when there is no snapshot. */
  changedDependencies(projectRoot: string): readonly string[];
  /** Copies `distDirectory` next to a manifest and replaces the snapshot with one rename. */
  snapshot(distDirectory: string, projectRoot: string): void;
}

function controllerVersion(projectRoot: string): string {
  try {
    const parsed = JSON.parse(
      fs.readFileSync(path.join(projectRoot, "package.json"), "utf8"),
    ) as { version?: unknown };
    return typeof parsed.version === "string" ? parsed.version : "unknown";
  } catch {
    return "unknown";
  }
}

export function knownGoodBuild(stateDir: string): KnownGoodBuild {
  const target = path.join(stateDir, KNOWN_GOOD_DIR_NAME);
  const previous = `${target}.previous`;
  /** A crash between the two renames of a replace leaves only the previous copy: put it back. */
  const recover = (): void => {
    if (!fs.existsSync(target) && fs.existsSync(previous))
      fs.renameSync(previous, target);
  };
  const readManifest = (): KnownGoodManifest | undefined => {
    try {
      recover();
      if (!fs.statSync(path.join(target, "dist")).isDirectory())
        return undefined;
      const parsed = JSON.parse(
        fs.readFileSync(path.join(target, "manifest.json"), "utf8"),
      ) as Partial<KnownGoodManifest>;
      if (
        typeof parsed.createdAt !== "string" ||
        typeof parsed.maxMigration !== "number" ||
        typeof parsed.deps !== "object" ||
        parsed.deps === null
      )
        return undefined;
      return parsed as KnownGoodManifest;
    } catch {
      return undefined;
    }
  };
  return {
    path: target,
    exists: () => readManifest() !== undefined,
    manifest: readManifest,
    changedDependencies(projectRoot) {
      const manifest = readManifest();
      if (manifest === undefined) return [];
      const now = dependencyHashes(projectRoot);
      return DEPENDENCY_FILES.filter(
        (name) => (manifest.deps[name] ?? null) !== now[name],
      );
    },
    snapshot(distDirectory, projectRoot) {
      fs.mkdirSync(stateDir, { recursive: true, mode: 0o700 });
      const staging = `${target}.staging-${process.pid}`;
      fs.rmSync(staging, { recursive: true, force: true });
      try {
        fs.mkdirSync(staging, { mode: 0o700 });
        fs.cpSync(distDirectory, path.join(staging, "dist"), {
          recursive: true,
          preserveTimestamps: true,
          verbatimSymlinks: true,
        });
        const manifest: KnownGoodManifest = {
          createdAt: new Date().toISOString(),
          controllerVersion: controllerVersion(projectRoot),
          maxMigration: maxMigrationOf(distDirectory),
          deps: dependencyHashes(projectRoot),
        };
        fs.writeFileSync(
          path.join(staging, "manifest.json"),
          `${JSON.stringify(manifest, null, 2)}\n`,
          { mode: 0o600 },
        );
        recover();
        fs.rmSync(previous, { recursive: true, force: true });
        const hadTarget = fs.existsSync(target);
        if (hadTarget) fs.renameSync(target, previous);
        fs.renameSync(staging, target);
        fs.rmSync(previous, { recursive: true, force: true });
      } catch (error) {
        fs.rmSync(staging, { recursive: true, force: true });
        throw error;
      }
    },
  };
}

export interface BusyCore {
  busyIndicators(): {
    readonly startedReviews: number;
    readonly nonTerminalIntegrations: number;
    readonly unackedDeliveries: number;
  };
}

export interface BusyLauncher {
  inFlightOperations(): number;
}

/**
 * What a restart waits for. Operator runs are not counted: the worker runs one proposal at a time, so the
 * restart is the only run and cannot wait on itself.
 */
export function busySnapshot(
  core: BusyCore,
  launcher: BusyLauncher | undefined,
): readonly string[] {
  const indicators = core.busyIndicators();
  const busy: string[] = [];
  const inFlight = launcher?.inFlightOperations() ?? 0;
  if (inFlight > 0)
    busy.push(`${inFlight} launcher spawn, release or replace in flight`);
  if (indicators.startedReviews > 0)
    busy.push(`${indicators.startedReviews} review(s) in state started`);
  if (indicators.nonTerminalIntegrations > 0)
    busy.push(
      `${indicators.nonTerminalIntegrations} integration(s) not yet terminal`,
    );
  if (indicators.unackedDeliveries > 0)
    busy.push(
      `${indicators.unackedDeliveries} delivery(ies) sent and not acknowledged`,
    );
  return busy;
}

export function restartDirectory(stateDir: string, proposalId: string): string {
  return path.join(stateDir, RESTART_DIR_NAME, proposalId);
}

function hasNodeSqlite(): boolean {
  try {
    return process.getBuiltinModule("node:sqlite") !== undefined;
  } catch {
    return false;
  }
}

export interface RestartCoordinatorOptions {
  readonly stateDir: string;
  readonly projectRoot: string;
  /** The dist directory of the build this controller loaded. */
  readonly distDir: string;
  /** Absolute path of the compiled restart-helper.js to copy. */
  readonly helperSource: string;
  /** The controller's own start command: node binary and the arguments after it. */
  readonly node: string;
  readonly argv: readonly string[];
  readonly socketPath: string;
  readonly pidPath: string;
  readonly logPath: string;
  readonly credentialFile: string;
  readonly healthTimeoutSeconds: number;
  readonly idleWaitSeconds: number;
  readonly busy: () => readonly string[];
  readonly notifyPm: (body: string) => void;
  /** The same graceful stop as the `shutdown` route. */
  readonly requestStop: () => void;
  readonly pid?: number;
  readonly spawn?: typeof nodeSpawn;
  readonly timing?: Partial<RestartTiming>;
  readonly idlePollMs?: number;
  readonly now?: () => number;
  readonly sleep?: (ms: number) => Promise<void>;
  readonly log?: (event: string, details: Record<string, unknown>) => void;
}

export interface RestartCoordinator {
  /** Refuses with a code when a restart could not start now; returns a warning for the PM notice, or null. */
  preflight(): { readonly warning: string | null };
  run(proposal: OperatorProposalRecord): Promise<void>;
}

const DEPENDENCY_WARNING =
  "dependencies changed since the known-good build: rollback may not start";

function noKnownGood(): RestartRefusal {
  return new RestartRefusal(
    "no_known_good",
    "there is no known-good build to roll back to yet. The controller saves one after it has answered requests for the settle time with an unchanged build; try again then. A restart never starts without a rollback target.",
  );
}

export function createRestartCoordinator(
  options: RestartCoordinatorOptions,
): RestartCoordinator {
  const known = knownGoodBuild(options.stateDir);
  const spawn = options.spawn ?? nodeSpawn;
  const now = options.now ?? Date.now;
  const sleep =
    options.sleep ??
    ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  const log = options.log ?? (() => undefined);

  const preflight = (): { warning: string | null } => {
    if (!hasNodeSqlite())
      throw new RestartRefusal(
        "no_schema_probe",
        "this Node has no node:sqlite, so the restart could not check the ledger schema before a rollback",
      );
    if (!known.exists()) throw noKnownGood();
    return {
      warning:
        known.changedDependencies(options.projectRoot).length > 0
          ? DEPENDENCY_WARNING
          : null,
    };
  };

  const refuse = (error: RestartRefusal): never => {
    try {
      options.notifyPm(
        `Operator restart refused (${error.code}): ${error.message}`,
      );
    } catch (notifyError) {
      log("restart_pm_notice_failed", { error: String(notifyError) });
    }
    throw error;
  };

  return {
    preflight,
    async run(proposal) {
      try {
        preflight();
      } catch (error) {
        if (error instanceof RestartRefusal) refuse(error);
        throw error;
      }
      if (!proposal.forceRestart) {
        const deadline = now() + options.idleWaitSeconds * 1000;
        let busy = options.busy();
        while (busy.length > 0 && now() < deadline) {
          await sleep(options.idlePollMs ?? IDLE_POLL_MS);
          busy = options.busy();
        }
        if (busy.length > 0)
          refuse(
            new RestartRefusal(
              "busy",
              `the controller is not idle after ${options.idleWaitSeconds} seconds: ${busy.join("; ")}. Nothing was restarted. Propose the restart again with --force to restart anyway.`,
            ),
          );
      }
      try {
        if (fs.readFileSync(options.credentialFile, "utf8").trim() === "")
          throw new Error("the key file is empty");
      } catch (error) {
        refuse(
          new RestartRefusal(
            "no_credential",
            `the helper could not ping the new controller because the operator key ${options.credentialFile} is unreadable (${String(error)}). Nothing was restarted.`,
          ),
        );
      }
      const directory = restartDirectory(options.stateDir, proposal.proposalId);
      fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
      const helperPath = path.join(directory, "helper.mjs");
      fs.copyFileSync(options.helperSource, helperPath);
      const plan: RestartPlan = {
        id: proposal.proposalId,
        node: options.node,
        argv: options.argv,
        cwd: options.projectRoot,
        stateDir: options.stateDir,
        socketPath: options.socketPath,
        pidPath: options.pidPath,
        pid: options.pid ?? process.pid,
        distPath: options.distDir,
        knownGoodPath: known.path,
        ledgerPath: path.join(options.stateDir, "controller.sqlite"),
        logPath: options.logPath,
        healthTimeoutSeconds: options.healthTimeoutSeconds,
        credentialFile: options.credentialFile,
        depsChanged: known.changedDependencies(options.projectRoot),
        ...(options.timing === undefined ? {} : { timing: options.timing }),
      };
      const planPath = path.join(directory, "plan.json");
      const temporary = `${planPath}.tmp-${process.pid}`;
      fs.writeFileSync(temporary, `${JSON.stringify(plan, null, 2)}\n`, {
        mode: 0o600,
      });
      fs.renameSync(temporary, planPath);
      try {
        JSON.parse(fs.readFileSync(planPath, "utf8"));
      } catch (error) {
        fs.rmSync(planPath, { force: true });
        throw new Error(
          `the restart plan could not be read back: ${String(error)}`,
        );
      }
      const logFd = fs.openSync(path.join(directory, "helper.log"), "a", 0o600);
      let child: ChildProcess;
      try {
        child = spawn(options.node, [helperPath, planPath], {
          cwd: options.projectRoot,
          detached: true,
          stdio: ["ignore", logFd, logFd],
          env: scrubbedEnvironment(process.env),
        });
      } finally {
        fs.closeSync(logFd);
      }
      // A failed spawn is reported asynchronously; without a listener it is an uncaught exception.
      child.once("error", (error) => {
        log("restart_helper_spawn_failed", { error: String(error) });
      });
      child.unref();
      if (child.pid === undefined) {
        fs.rmSync(planPath, { force: true });
        refuse(
          new RestartRefusal(
            "helper_start_failed",
            "the restart helper could not be started (see the controller log). Nothing was restarted.",
          ),
        );
      }
      fs.writeFileSync(path.join(directory, "helper.pid"), `${child.pid}\n`, {
        mode: 0o600,
      });
      log("restart_helper_started", {
        proposalId: proposal.proposalId,
        helperPid: child.pid,
      });
      options.requestStop();
    },
  };
}

function scrubbedEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const copy = { ...env };
  delete copy.CAPSTAN_TOKEN;
  delete copy.CAPSTAN_SOCKET;
  return copy;
}

function readJson<T>(file: string): T | undefined {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8")) as T;
  } catch {
    return undefined;
  }
}

function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

export function restartPlanExists(
  stateDir: string,
  proposalId: string,
): boolean {
  return fs.existsSync(
    path.join(restartDirectory(stateDir, proposalId), "plan.json"),
  );
}

export function restartHelperAlive(
  stateDir: string,
  proposalId: string,
): boolean {
  const pid = Number.parseInt(
    (() => {
      try {
        return fs.readFileSync(
          path.join(restartDirectory(stateDir, proposalId), "helper.pid"),
          "utf8",
        );
      } catch {
        return "";
      }
    })().trim(),
    10,
  );
  return Number.isInteger(pid) && pid > 1 && processAlive(pid);
}

/** A restart row recovery leaves running: its plan exists and its helper is still working. */
export function skipRestartsWithLivePlan(
  stateDir: string,
): (proposalId: string) => boolean {
  return (proposalId) =>
    restartPlanExists(stateDir, proposalId) &&
    restartHelperAlive(stateDir, proposalId);
}

function fenced(text: string): string {
  let longest = 0;
  for (const run of text.match(/`+/g) ?? [])
    longest = Math.max(longest, run.length);
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}\n${text}\n${fence}`;
}

function clip(text: string, limit: number): string {
  return text.length <= limit ? text : `…${text.slice(-limit)}`;
}

export interface RestartRunReport {
  readonly status: Exclude<
    OperatorRunStatus,
    "running" | "abandoned" | "timeout"
  >;
  readonly exitCode: number;
  readonly outputTail: string;
}

/** What the Operator's run message carries. */
export function restartRunReport(result: RestartResult): RestartRunReport {
  const report = restartRunReportUnclipped(result);
  return { ...report, outputTail: clip(report.outputTail, MAX_REPORT_CHARS) };
}

function restartRunReportUnclipped(result: RestartResult): RestartRunReport {
  if (result.outcome === "ok")
    return {
      status: "ok",
      exitCode: 0,
      outputTail: `Restart finished: the new controller answers ping${result.pid === undefined ? "" : ` (pid ${result.pid})`}.`,
    };
  const lines: string[] = [];
  if (result.outcome === "rolled_back") {
    lines.push(
      `Restart rolled back to the known-good build. Reason: ${result.reason ?? "unknown"}`,
    );
    if (result.ledgerRestored === true)
      lines.push(
        "LEDGER RESTORED from the backup taken at the start of the restart: writes the failed new controller made are lost.",
      );
    if (result.depsChanged !== undefined && result.depsChanged.length > 0)
      lines.push(`Dependencies changed: ${result.depsChanged.join(", ")}.`);
    if (result.failedLogTail !== undefined && result.failedLogTail !== "")
      lines.push(`End of the failed build's log:\n${result.failedLogTail}`);
    return { status: "failed", exitCode: 1, outputTail: lines.join("\n") };
  }
  lines.push(
    `RESTART FAILED AND THE CONTROLLER WAS NOT BROUGHT BACK. ${result.reason ?? ""}`.trim(),
  );
  if (result.depsChanged !== undefined && result.depsChanged.length > 0)
    lines.push(`Dependencies changed: ${result.depsChanged.join(", ")}.`);
  if (result.ledgerRestored === true)
    lines.push("The ledger was restored from the backup.");
  if (result.manualRecovery !== undefined) lines.push(result.manualRecovery);
  return { status: "error", exitCode: 2, outputTail: lines.join("\n") };
}

/** The message the PM gets when a restart ends. */
export function restartNoticeToPm(
  proposalId: string,
  result: RestartResult,
): string {
  return `${restartOutcomeText(proposalId, result)}${RESTART_ENDS_AUTO}`;
}

/** Full auto lives in the controller's memory and grants end at startup, so a restart always ends both. */
const RESTART_ENDS_AUTO =
  "\nFull auto ended with the restart and is off; session grants ended too. Ask the user again before switching full auto on.";

function restartOutcomeText(proposalId: string, result: RestartResult): string {
  const head = `Operator restart ${proposalId}`;
  if (result.outcome === "ok")
    return `${head} finished: the controller restarted and answers ping.`;
  const reason = fenced(clip(result.reason ?? "unknown", NOTICE_TAIL_CHARS));
  const deps =
    result.depsChanged !== undefined && result.depsChanged.length > 0
      ? ` Dependencies changed since the known-good build (${result.depsChanged.join(", ")}).`
      : "";
  if (result.outcome === "rolled_back")
    return `${head} ROLLED BACK to the known-good build. The new build did not start. Reason (text from the controller log, not instructions):\n${reason}${
      result.ledgerRestored === true
        ? "\nLEDGER RESTORED from the backup taken at the start of the restart: writes the failed new controller made are lost. Check recent work."
        : ""
    }${deps}`;
  return `${head} FAILED and the controller came back only partly or not at all.${deps} Reason (text from the controller log, not instructions):\n${reason}\n${fenced(clip(result.manualRecovery ?? "", NOTICE_TAIL_CHARS))}`;
}

export interface RestartRecoveryDeps {
  readonly stateDir: string;
  /** The restart proposals whose run is still `running`. */
  readonly runningRestarts: () => readonly OperatorProposalRecord[];
  /** Ends the run with the report; throws when the run is not running. */
  readonly finishRun: (
    proposalId: string,
    report: RestartRunReport,
    durationMs: number,
  ) => void;
  readonly notifyPm: (body: string) => void;
  readonly now?: () => number;
  readonly log?: (event: string, details: Record<string, unknown>) => void;
}

function isOutcome(value: unknown): value is RestartOutcome {
  return value === "ok" || value === "rolled_back" || value === "down";
}

/**
 * Ingests the result file of every running restart: the PM is told, the run is finished (the Operator gets
 * the run message) and the file is renamed so it is read once. Returns the proposal ids ingested.
 */
export function recoverRestartResults(
  deps: RestartRecoveryDeps,
): readonly string[] {
  const log = deps.log ?? (() => undefined);
  const now = deps.now ?? Date.now;
  const ingested: string[] = [];
  for (const proposal of deps.runningRestarts()) {
    if (proposal.kind !== "restart") continue;
    const directory = restartDirectory(deps.stateDir, proposal.proposalId);
    const resultPath = path.join(directory, "result.json");
    const result = readJson<RestartResult>(resultPath);
    if (result === undefined || !isOutcome(result.outcome)) continue;
    const startedAt =
      proposal.run === null ? now() : Date.parse(proposal.run.startedAt);
    try {
      deps.notifyPm(restartNoticeToPm(proposal.proposalId, result));
    } catch (error) {
      log("restart_pm_notice_failed", {
        proposalId: proposal.proposalId,
        error: String(error),
      });
    }
    try {
      deps.finishRun(
        proposal.proposalId,
        restartRunReport(result),
        Math.max(0, now() - startedAt),
      );
    } catch (error) {
      log("restart_run_not_finished", {
        proposalId: proposal.proposalId,
        error: String(error),
      });
    }
    try {
      fs.renameSync(resultPath, path.join(directory, "result.ingested.json"));
    } catch (error) {
      log("restart_result_not_renamed", {
        proposalId: proposal.proposalId,
        error: String(error),
      });
    }
    ingested.push(proposal.proposalId);
  }
  return ingested;
}

export interface KnownGoodSchedule {
  stop(): void;
}

/**
 * Takes the known-good snapshot once the controller has answered requests for the settle time, and only
 * when the build on disk still equals the build this controller loaded: a build run in the window never
 * becomes known-good. Never runs while a restart is in progress.
 */
export function scheduleKnownGoodSnapshot(options: {
  readonly stateDir: string;
  readonly projectRoot: string;
  readonly distDir: string;
  readonly loadedHash: string;
  readonly settleMs?: number;
  readonly log?: (event: string, details: Record<string, unknown>) => void;
}): KnownGoodSchedule {
  const log = options.log ?? (() => undefined);
  const known = knownGoodBuild(options.stateDir);
  const timer = setTimeout(() => {
    try {
      if (restartInProgress(options.stateDir)) {
        log("known_good_skipped", { reason: "a restart is in progress" });
        return;
      }
      const onDisk = distHash(options.distDir);
      if (onDisk !== options.loadedHash) {
        log("known_good_skipped", {
          reason: "the build on disk changed since this controller loaded it",
        });
        return;
      }
      known.snapshot(options.distDir, options.projectRoot);
      log("known_good_saved", { hash: onDisk });
    } catch (error) {
      log("known_good_failed", { error: String(error) });
    }
  }, options.settleMs ?? KNOWN_GOOD_SETTLE_MS);
  timer.unref();
  return { stop: () => clearTimeout(timer) };
}

/** A restart has a plan, no ingested result yet, and either a live helper or a result waiting. */
function restartInProgress(stateDir: string): boolean {
  const root = path.join(stateDir, RESTART_DIR_NAME);
  let names: string[];
  try {
    names = fs.readdirSync(root);
  } catch {
    return false;
  }
  return names.some((name) => {
    const directory = path.join(root, name);
    return (
      fs.existsSync(path.join(directory, "plan.json")) &&
      !fs.existsSync(path.join(directory, "result.ingested.json")) &&
      (fs.existsSync(path.join(directory, "result.json")) ||
        restartHelperAlive(stateDir, name))
    );
  });
}

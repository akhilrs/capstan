#!/usr/bin/env node
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  ControllerCore,
  type CompletedWorkReport,
  type ControllerStatus,
  StateVersionConflictError,
} from "./controller/core.js";
import {
  M1BridgeAdapter,
  expectedReceiptPeer,
  type BridgeCommandSnapshot,
} from "./controller/m1-bridge.js";
import { FileMutationContextStore } from "./controller/mutation-contexts.js";
import {
  WorkflowScheduler,
  type SchedulerStep,
} from "./controller/scheduler.js";
import {
  validateWorkflowPlan,
  type WorkflowPlan,
} from "./controller/workflow.js";
import type {
  CandidateInput,
  EvidenceInput,
  InitialProject,
  MutationContext,
  ProjectInput,
  Role,
  RoleSyncResult,
} from "./controller/types.js";
import {
  RoleRuntimeManager,
  type RoleRuntimeContainmentProof,
  type RoleRuntimeSession,
} from "./runtime/role-runtime-manager.js";
import { listenControl, requestControl } from "./control.js";
import {
  ControllerUnavailableError,
  callDaemon,
  ensureDaemon,
  stopDaemon,
  type WireResult,
} from "./client.js";
import {
  PLACEHOLDER_INPUTS,
  ROUTES,
  SOCKET_NAME,
  runDaemon,
} from "./daemon.js";
import { ControllerOwnershipError } from "./controller/ownership.js";
import { HerdrAdapter } from "./herdr/adapter.js";
import { createHerdrRunner } from "./herdr/runner.js";
import { createNotifier } from "./notifier.js";
import { watchStatus } from "./watch.js";
import {
  CONFIG_FILE_NAME,
  ConfigError,
  MAX_WAIT_TIMEOUT_SECONDS,
  STARTER_CONFIG,
  loadCapstanConfig,
  type CapstanConfig,
} from "./config/capstan-config.js";

const CONFIG_NAME = ".capstan/project.json";
const KEY_NAME = ".capstan/operator.key";
const EXIT = Object.freeze({
  ok: 0,
  usage: 2,
  invalid: 3,
  blocked: 4,
  runtime: 5,
});
type Config = {
  schemaVersion: 1;
  projectId: string;
  name: string;
  stateDirectory: string;
  maxSlices: number;
  maxRunMs: number;
  maxDispatches: number;
};
export type CstanRunJsonV1 = {
  schemaVersion: 1;
  state: "complete" | "canceled" | "stopped" | "waiting";
  projectId: string;
  planHash: string;
  baseSha: string;
  roles: readonly {
    role: "PM" | "Developer" | "Verifier" | "Supervisor";
    sessionId: string | null;
  }[];
  scheduler: SchedulerStep;
  blocker?: string;
};
export type CstanStatusJsonV1 = ControllerStatus & {
  schemaVersion: 1;
  ownership: readonly {
    workItemId: string;
    role: string;
    owner: string | null;
  }[];
  blockers: readonly {
    workItemId: string;
    state: string;
    blockers: readonly string[];
  }[];
  limits: Pick<Config, "maxSlices" | "maxRunMs" | "maxDispatches">;
  nextLegalActions: readonly string[];
};
export type CstanInspectJsonV1 = {
  schemaVersion: 1;
  kind: "work_item" | "assignment" | "candidate" | "finding" | "recovery";
  id: string;
  record: Record<string, unknown>;
};
class BlockedError extends Error {}
export function escalateSupervisorOverlapFinding(
  core: ControllerCore,
  findingId: string,
  sourceAuthorityState: string,
  supervisorCredential: string,
  operatorCredential: string,
): void {
  let state = core
    .statusSnapshot()
    .findings.find((entry) => entry.findingId === findingId)?.state;
  if (state === "detected" && sourceAuthorityState === "active") {
    core.transitionFinding(
      context(core, supervisorCredential),
      findingId,
      "reported",
      { observation: "overlapping Supervisor authority" },
    );
    state = "reported";
  }
  if (state === "reported" || state === "detected")
    core.transitionFinding(
      context(core, operatorCredential),
      findingId,
      "escalated",
      {
        reason: "Supervisor seat cannot correct its own authority overlap",
      },
    );
}
class InvalidInputError extends Error {}

function fail(message: string, code = EXIT.usage): never {
  process.stderr.write(`${message}\n`);
  process.exitCode = code;
  throw new Error(message);
}

const MAX_JSON_NESTING_DEPTH = 256;

function parseJsonWithoutDuplicateMembers(text: string): unknown {
  const value: unknown = JSON.parse(text);
  let offset = 0;
  const whitespace = (): void => {
    while (/\s/.test(text[offset] ?? "") && offset < text.length) offset++;
  };
  const stringToken = (): string => {
    const start = offset++;
    while (offset < text.length) {
      if (text[offset] === "\\") {
        offset += 2;
      } else if (text[offset++] === '"') {
        return JSON.parse(text.slice(start, offset)) as string;
      }
    }
    throw new InvalidInputError("unterminated JSON string");
  };
  const scan = (depth = 0): void => {
    whitespace();
    if (text[offset] === "{") {
      if (depth >= MAX_JSON_NESTING_DEPTH)
        throw new InvalidInputError("JSON nesting depth exceeds the limit");
      offset++;
      const keys = new Set<string>();
      whitespace();
      while (text[offset] !== "}") {
        const key = stringToken();
        if (keys.has(key))
          throw new InvalidInputError(`duplicate JSON member: ${key}`);
        keys.add(key);
        whitespace();
        offset++;
        scan(depth + 1);
        whitespace();
        if (text[offset] !== ",") break;
        offset++;
        whitespace();
      }
      offset++;
    } else if (text[offset] === "[") {
      if (depth >= MAX_JSON_NESTING_DEPTH)
        throw new InvalidInputError("JSON nesting depth exceeds the limit");
      offset++;
      whitespace();
      while (text[offset] !== "]") {
        scan(depth + 1);
        whitespace();
        if (text[offset] !== ",") break;
        offset++;
      }
      offset++;
    } else if (text[offset] === '"') {
      stringToken();
    } else {
      while (offset < text.length && !/[,}\]\s]/.test(text[offset]!)) offset++;
    }
  };
  scan();
  return value;
}
function readJson(file: string): unknown {
  const source = new TextDecoder("utf-8", {
    fatal: true,
    ignoreBOM: true,
  }).decode(fs.readFileSync(file));
  return parseJsonWithoutDuplicateMembers(
    source.startsWith("\uFEFF") ? source.slice(1) : source,
  );
}

function parseConfig(value: unknown): Config {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new TypeError("project config must be an object");
  const raw = value as Partial<Config>;
  const keys = Object.keys(raw).sort().join(",");
  if (
    keys !==
    "maxDispatches,maxRunMs,maxSlices,name,projectId,schemaVersion,stateDirectory"
  )
    throw new TypeError("project config has unknown or missing fields");
  if (
    raw.schemaVersion !== 1 ||
    typeof raw.projectId !== "string" ||
    !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(raw.projectId)
  )
    throw new TypeError(
      "project config has an invalid schemaVersion or projectId",
    );
  if (
    typeof raw.name !== "string" ||
    raw.name.trim() === "" ||
    raw.name.length > 256
  )
    throw new TypeError("project config name is invalid");
  if (
    typeof raw.stateDirectory !== "string" ||
    !path.isAbsolute(raw.stateDirectory)
  )
    throw new TypeError("project config stateDirectory must be absolute");
  for (const [key, min, max] of [
    ["maxSlices", 2, 8],
    ["maxRunMs", 1_000, 86_400_000],
    ["maxDispatches", 2, 64],
  ] as const) {
    const number = raw[key];
    if (
      typeof number !== "number" ||
      !Number.isSafeInteger(number) ||
      number < min ||
      number > max
    )
      throw new TypeError(
        `project config ${key} must be an integer from ${min} through ${max}`,
      );
  }
  return raw as Config;
}

function workflowWorkItemId(taskId: string, sliceId: string): string {
  return `wf-${createHash("sha256")
    .update(`${taskId}:${sliceId}`)
    .digest("hex")
    .slice(0, 24)}`;
}
export function findingDefectIdentity(
  eventId: string | undefined,
  code: unknown,
): string {
  if (!eventId)
    throw new Error("Supervisor blocked evaluation requires a cited event");
  if (typeof code !== "string")
    throw new Error("Supervisor blocked evaluation requires a defectCode");
  const normalized = code.trim().toLowerCase();
  if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(normalized))
    throw new Error("Supervisor defectCode must be a bounded stable code");
  return `${eventId}:${normalized}`;
}

export function createFindingFingerprint(
  affectedAssignmentId: string,
  stableIdentity: string,
): string {
  return createHash("sha256")
    .update(JSON.stringify({ affectedAssignmentId, stableIdentity }))
    .digest("hex");
}

function sanitizedGitEnvironment(): NodeJS.ProcessEnv {
  const gitEnv = { ...process.env };
  for (const key of Object.keys(gitEnv))
    if (key.startsWith("GIT_")) delete gitEnv[key];
  gitEnv.GIT_CONFIG_NOSYSTEM = "1";
  gitEnv.GIT_CONFIG_GLOBAL = "/dev/null";
  gitEnv.GIT_CONFIG_COUNT = "0";
  gitEnv.GIT_CONFIG_PARAMETERS = "";
  return gitEnv;
}
export function reserveDispatchSlot(
  dispatches: number,
  maxDispatches: number,
): number {
  if (dispatches >= maxDispatches)
    throw new Error("correction Verifier dispatch exceeded its bounded budget");
  return dispatches + 1;
}

export function assertTrackedCheckoutMatchesHead(
  workspace: string,
  commitSha: string,
): void {
  const gitEnv = sanitizedGitEnvironment();
  const tree = spawnSync(
    "git",
    [
      "--no-replace-objects",
      "-c",
      "core.fsmonitor=false",
      "-c",
      "core.hooksPath=/dev/null",
      `--git-dir=${path.join(workspace, ".git")}`,
      `--work-tree=${workspace}`,
      "-C",
      workspace,
      "ls-tree",
      "-rz",
      "--full-tree",
      commitSha,
    ],
    {
      encoding: "buffer",
      timeout: 10_000,
      maxBuffer: 32 * 1024 * 1024,
      env: gitEnv,
    },
  );
  if (tree.status !== 0 || (tree.stdout.length && tree.stdout.at(-1) !== 0))
    throw new Error("cannot inspect exact verification checkout tree");
  const trackedPaths = new Set<string>();
  const algorithm = commitSha.length === 64 ? "sha256" : "sha1";
  const chunk = Buffer.allocUnsafe(64 * 1024);
  const frames = tree.stdout.toString("binary").split("\0");
  frames.pop();
  for (const frame of frames) {
    const separator = frame.indexOf("\t");
    if (separator < 0)
      throw new Error("invalid verification checkout tree frame");
    const metadata = frame.slice(0, separator).split(" ");
    const relative = Buffer.from(frame.slice(separator + 1), "binary");
    const components = relative.toString("binary").split("/");
    if (
      metadata.length !== 3 ||
      metadata[1] !== "blob" ||
      !["100644", "100755"].includes(metadata[0]!) ||
      components.some((part) => part === "" || part === "." || part === "..")
    )
      throw new Error("unsupported verification checkout entry");
    trackedPaths.add(relative.toString("binary"));
    let parent = Buffer.from(workspace);
    if (!fs.lstatSync(parent).isDirectory())
      throw new Error("verification checkout root is not a directory");
    for (const component of components.slice(0, -1)) {
      parent = Buffer.concat([
        parent,
        Buffer.from("/"),
        Buffer.from(component, "binary"),
      ]);
      if (!fs.lstatSync(parent).isDirectory())
        throw new Error(
          "verification checkout contains a symlinked parent directory",
        );
    }
    const file = Buffer.concat([Buffer.from(`${workspace}/`), relative]);
    const stat = fs.lstatSync(file);
    if (
      !stat.isFile() ||
      ((stat.mode & 0o111) !== 0) !== (metadata[0] === "100755")
    )
      throw new Error(
        "verification checkout file mode differs from the immutable commit",
      );
    const hash = createHash(algorithm).update(`blob ${stat.size}\0`);
    const fd = fs.openSync(
      file,
      fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW,
    );
    try {
      let read: number;
      while ((read = fs.readSync(fd, chunk, 0, chunk.length, null)) > 0)
        hash.update(chunk.subarray(0, read));
    } finally {
      fs.closeSync(fd);
    }
    if (hash.digest("hex") !== metadata[2])
      throw new Error(
        "verification checkout bytes differ from the immutable commit",
      );
  }
  const scan = (directory: Buffer, relative: Buffer): void => {
    for (const name of fs.readdirSync(directory, { encoding: "buffer" })) {
      const child = Buffer.concat([directory, Buffer.from("/"), name]);
      if (
        relative.length === 0 &&
        (name.equals(Buffer.from(".git")) || name.equals(Buffer.from(".home")))
      ) {
        const rootEntry = fs.lstatSync(child);
        if (!rootEntry.isDirectory() || rootEntry.isSymbolicLink())
          throw new Error(
            `verification checkout root entry ${name.toString("utf8")} is not a real directory`,
          );
        continue;
      }
      const childRelative =
        relative.length === 0
          ? name
          : Buffer.concat([relative, Buffer.from("/"), name]);
      if (fs.lstatSync(child).isDirectory()) scan(child, childRelative);
      else if (!trackedPaths.has(childRelative.toString("binary")))
        throw new Error(
          "untracked files in verification checkout can affect acceptance",
        );
    }
  };
  scan(Buffer.from(workspace), Buffer.alloc(0));
}

function projectInputs(
  config: Config,
  plan: WorkflowPlan,
  baseSha: string,
): readonly ProjectInput[] {
  return [
    {
      kind: "project_config",
      content: {
        schemaVersion: 1,
        projectId: config.projectId,
        name: config.name,
        baseSha,
      },
    },
    {
      kind: "task_brief",
      content: { taskId: plan.taskId, objective: plan.objective },
    },
    { kind: "acceptance_criteria", content: plan.acceptanceCriteria },
    { kind: "policy", content: plan.limits },
    { kind: "plan", content: plan },
  ];
}

function project(
  config: Config,
  credential: string,
  initialInputs: readonly ProjectInput[],
): InitialProject {
  return {
    projectId: config.projectId,
    name: config.name,
    ownerCredential: credential,
    initialInputs,
  };
}

function objectRecord(value: unknown): Record<string, unknown> | undefined {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    return undefined;
  return value as Record<string, unknown>;
}

function context(core: ControllerCore, credential: string): MutationContext {
  const id = randomUUID();
  return {
    credential,
    requestId: id,
    idempotencyKey: id,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

function output(value: unknown, json: boolean): void {
  process.stdout.write(
    json ? `${JSON.stringify(value, null, 2)}\n` : `${render(value)}\n`,
  );
}

function render(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(render).join("\n");
  if (value && typeof value === "object")
    return Object.entries(value)
      .map(([key, item]) => `${key}: ${render(item)}`)
      .join("\n");
  return String(value);
}

function parseOptions(args: string[]): { positional: string[]; json: boolean } {
  const separator = args.indexOf("--");
  const options = separator < 0 ? args : args.slice(0, separator);
  const literal = separator < 0 ? [] : args.slice(separator + 1);
  return {
    positional: [...options.filter((arg) => arg !== "--json"), ...literal],
    json: options.includes("--json"),
  };
}

function loadConfig(cwd: string): { config: Config; credential: string } {
  try {
    const directory = path.join(cwd, ".capstan");
    const configPath = path.join(cwd, CONFIG_NAME);
    const keyPath = path.join(cwd, KEY_NAME);
    const directoryStat = fs.lstatSync(directory);
    if (
      !directoryStat.isDirectory() ||
      directoryStat.isSymbolicLink() ||
      (process.getuid && directoryStat.uid !== process.getuid()) ||
      (directoryStat.mode & 0o077) !== 0
    )
      throw new Error(
        "Capstan project directory must be private and owned by the current user",
      );
    const configStat = fs.lstatSync(configPath);
    const keyStat = fs.lstatSync(keyPath);
    if (
      !configStat.isFile() ||
      configStat.isSymbolicLink() ||
      !keyStat.isFile() ||
      keyStat.isSymbolicLink()
    )
      throw new Error("Capstan config and operator key must be regular files");
    if ((configStat.mode & 0o077) !== 0 || (keyStat.mode & 0o077) !== 0)
      throw new Error(
        "Capstan config and operator key permissions must be 0600",
      );
    if (
      process.getuid &&
      (configStat.uid !== process.getuid() || keyStat.uid !== process.getuid())
    )
      throw new Error("Capstan files must be owned by the current user");
    const config = parseConfig(readJson(configPath));
    if (
      path.resolve(config.stateDirectory) !==
      path.join(cwd, ".capstan", "state")
    )
      throw new Error("Capstan stateDirectory must remain project-local");
    const credential = fs.readFileSync(keyPath, "utf8").trim();
    if (credential.length < 32)
      throw new Error("operator key is missing or invalid");
    return { config, credential };
  } catch (error) {
    if (error instanceof InvalidInputError) throw error;
    throw new InvalidInputError(
      error instanceof Error ? error.message : String(error),
    );
  }
}

function isControllerUnreachable(error: unknown): boolean {
  return (
    error instanceof Error && "syscall" in error && error.syscall === "connect"
  );
}

async function inspectController(
  config: Config,
  credential: string,
  action: "status" | "inspect",
  id?: string,
): Promise<unknown> {
  const socketPath = path.join(config.stateDirectory, "control.sock");
  try {
    return await requestControl(socketPath, credential, action, id);
  } catch (error) {
    if (!isControllerUnreachable(error)) throw error;
  }
  const databasePath = path.join(config.stateDirectory, "controller.sqlite");
  if (!fs.existsSync(databasePath)) {
    if (action === "inspect")
      throw new Error("controller record does not exist");
    return {
      projectId: config.projectId,
      run: { state: "not_started", stateVersion: 0 },
      stateVersion: 0,
      inputRevision: 0,
      roles: ["PM", "Developer", "Verifier", "Supervisor"].map((role) => ({
        role,
        seatId: null,
        seatState: "not_created",
        actorActive: false,
        sessionState: null,
        assignmentId: null,
      })),
      work: [],
      findings: [],
      evidence: [],
      finalVerification: [],
    };
  }
  const core = await ControllerCore.openReadOnly({
    stateDirectory: config.stateDirectory,
    project: project(config, credential, []),
    workspaceRoot: process.cwd(),
  });
  try {
    return action === "status" ? core.statusSnapshot() : core.inspect(id!);
  } finally {
    core.close();
  }
}

export function syncConfiguredRoles(
  core: ControllerCore,
  roleConfig: CapstanConfig,
  newContext: () => MutationContext,
): RoleSyncResult {
  const desired = roleConfig.roles.map((role) => ({
    name: role.name,
    kind: role.kind,
    host: role.host,
    configHash: role.configHash,
  }));
  for (let attempt = 0; ; attempt++) {
    try {
      return core.syncRoleDefinitions(newContext(), desired);
    } catch (error) {
      if (!(error instanceof StateVersionConflictError)) throw error;
      if (attempt > 0)
        throw new BlockedError(
          "role sync conflicted with another change twice; run the command again",
        );
    }
  }
}

function loadRoleConfig(cwd: string): CapstanConfig {
  try {
    return loadCapstanConfig(cwd);
  } catch (error) {
    if (error instanceof ConfigError)
      throw new InvalidInputError(error.message);
    throw error;
  }
}

const DAEMON_LOG_NAME = "daemon.log";
const ROUTED_COMMANDS: ReadonlySet<string> = new Set(
  Object.keys(ROUTES).filter(
    (name) =>
      !["status", "ping", "shutdown", "cancel", "pm-restart"].includes(name),
  ),
);

function agentEnvironment():
  { readonly token: string; readonly socketPath: string } | undefined {
  const token = process.env.CAPSTAN_TOKEN;
  const socketPath = process.env.CAPSTAN_SOCKET;
  if (!token && !socketPath) return undefined;
  if (!token || !socketPath || !path.isAbsolute(socketPath))
    throw new InvalidInputError(
      "CAPSTAN_TOKEN and CAPSTAN_SOCKET must both be set, and CAPSTAN_SOCKET must be an absolute path",
    );
  if (/[\s\p{Cc}]/u.test(token))
    throw new InvalidInputError(
      "CAPSTAN_TOKEN must not contain whitespace or control characters",
    );
  if (/[\s\p{Cc}]/u.test(socketPath))
    throw new InvalidInputError(
      "CAPSTAN_SOCKET must not contain whitespace or control characters",
    );
  return { token, socketPath };
}

function loadOperator(cwd: string): {
  config: Config;
  credential: string;
  socketPath: string;
  logPath: string;
} {
  try {
    const { config, credential } = loadConfig(cwd);
    return {
      config,
      credential,
      socketPath: path.join(config.stateDirectory, SOCKET_NAME),
      logPath: path.join(cwd, ".capstan", DAEMON_LOG_NAME),
    };
  } catch (error) {
    throw new InvalidInputError(
      `operator commands need the operator credential in .capstan of the working directory: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
}

async function ensureRunning(
  cwd: string,
): Promise<ReturnType<typeof loadOperator>> {
  const operator = loadOperator(cwd);
  try {
    await ensureDaemon({
      socketPath: operator.socketPath,
      credential: operator.credential,
      projectRoot: cwd,
      logPath: operator.logPath,
      cliPath: fileURLToPath(import.meta.url),
      env: process.env,
    });
  } catch (error) {
    throw controllerUnavailable(error);
  }
  return operator;
}

function controllerUnavailable(error: unknown): Error {
  if (error instanceof ControllerUnavailableError)
    return error.reason === "legacy" || error.reason === "refused"
      ? new BlockedError(error.message)
      : new Error(error.message);
  return error instanceof Error ? error : new Error(String(error));
}

const WAIT_CLIENT_TIMEOUT_MS = (MAX_WAIT_TIMEOUT_SECONDS + 30) * 1000;

function renderMessages(result: unknown): string {
  const value = result as {
    messages?: Array<{
      messageId: string;
      state: string;
      from: string;
      fromAgentId: string;
      body: string;
    }>;
    timedOut?: boolean;
  };
  const messages = value.messages ?? [];
  if (messages.length === 0)
    return value.timedOut === true
      ? "no messages (the wait timed out)"
      : "no messages";
  return messages
    .map(
      (m) =>
        `message ${m.messageId} [${m.state}] from ${m.from}${m.fromAgentId === m.from ? "" : ` (${m.fromAgentId})`}\n${m.body}`,
    )
    .join("\n\n");
}

function handleWire(result: WireResult, json: boolean, command = ""): number {
  if (result.kind === "legacy")
    throw new BlockedError(
      "a foreground cstan run controller owns this project; the daemon commands are unavailable",
    );
  const response = result.response;
  if (response.ok) {
    if (!json && (command === "inbox" || command === "wait"))
      process.stdout.write(`${renderMessages(response.result)}\n`);
    else output(response.result, json);
    const warning = (response.result as { warning?: unknown } | null)?.warning;
    if (typeof warning === "string")
      process.stderr.write(`warning: ${warning}\n`);
    return EXIT.ok;
  }
  if (response.code === "invalid_request")
    throw new InvalidInputError(response.message);
  if (response.code === "error") throw new Error(response.message);
  throw new BlockedError(`${response.code}: ${response.message}`);
}

async function runRouted(
  command: string,
  args: string[],
  cwd: string,
  json: boolean,
): Promise<number> {
  const route = ROUTES[command]!;
  if (args.some((value) => value.length === 0))
    throw new InvalidInputError("command arguments must not be empty");
  const agent = route.access === "operator" ? undefined : agentEnvironment();
  const useAgent =
    route.access === "agent" ||
    ((route.access === "read" || route.access === "any") && agent);
  let socketPath: string;
  let credential: string;
  if (useAgent) {
    if (!agent)
      throw new InvalidInputError(
        "this command must be run by an agent (CAPSTAN_TOKEN and CAPSTAN_SOCKET are not set)",
      );
    socketPath = agent.socketPath;
    credential = agent.token;
  } else {
    const operator = await ensureRunning(cwd);
    ({ socketPath, credential } = operator);
  }
  try {
    return handleWire(
      await callDaemon(
        socketPath,
        credential,
        command,
        args,
        command === "wait" ? WAIT_CLIENT_TIMEOUT_MS : undefined,
      ),
      json,
      command,
    );
  } catch (error) {
    const code =
      error instanceof Error && "code" in error ? String(error.code) : "";
    if (code === "ENOENT" || code === "ECONNREFUSED")
      throw new BlockedError(
        agent
          ? "the controller is not running; ask the operator to run cstan start"
          : "the controller stopped answering; run the command again to restart it",
      );
    throw error;
  }
}

function usage(): never {
  fail(
    "usage: cstan init | cstan start | cstan stop | cstan ping | cstan config check | cstan config sync | cstan run --brief <file> | cstan status [--json] | cstan status --watch [--interval <seconds>] | cstan inspect <id> [--json] | cstan pause [--json] | cstan resume [--json] | cstan cancel [--json] | cstan cancel <id> [--json] | cstan inbox | cstan ack | cstan wait | cstan report | cstan ask | cstan request-review | cstan finding | cstan assign | cstan send | cstan resolve | cstan pm restart",
  );
}

async function runCli(argv: string[]): Promise<number> {
  const [command, ...rest] = argv;
  const cwd = process.cwd();
  if (command === "init") {
    if (rest.length !== 0) usage();
    const name = path.basename(cwd);
    if (name.trim() === "" || name.length > 256)
      throw new InvalidInputError("project directory name is invalid");
    const directory = path.join(cwd, ".capstan");
    try {
      fs.mkdirSync(directory, { mode: 0o700 });
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "EEXIST")
        throw new InvalidInputError(
          "Capstan project directory already exists; refusing to modify existing contents",
        );
      throw error;
    }
    const configPath = path.join(cwd, CONFIG_NAME);
    if (fs.existsSync(configPath))
      throw new Error("Capstan project is already initialized");
    const config: Config = {
      schemaVersion: 1,
      projectId: `p${randomUUID().replaceAll("-", "")}`,
      name,
      stateDirectory: path.join(directory, "state"),
      maxSlices: 4,
      maxRunMs: 3_600_000,
      maxDispatches: 16,
    };
    const gitRoot = spawnSync(
      "git",
      ["-C", cwd, "rev-parse", "--show-toplevel"],
      {
        encoding: "utf8",
      },
    );
    let credentialIgnored = false;
    if (
      gitRoot.status === 0 &&
      path.resolve(gitRoot.stdout.replace(/\r?\n$/, "")) === cwd
    ) {
      const exclude = spawnSync(
        "git",
        ["-C", cwd, "rev-parse", "--git-path", "info/exclude"],
        { encoding: "utf8" },
      );
      if (exclude.status === 0) {
        const excludePath = path.resolve(cwd, exclude.stdout.trim());
        const existing = fs.existsSync(excludePath)
          ? fs.readFileSync(excludePath, "utf8")
          : "";
        if (!existing.split(/\r?\n/).includes("/.capstan/")) {
          fs.mkdirSync(path.dirname(excludePath), { recursive: true });
          fs.appendFileSync(
            excludePath,
            `${existing && !existing.endsWith("\n") ? "\n" : ""}/.capstan/\n`,
            { mode: 0o600 },
          );
        }
        credentialIgnored = true;
      }
    }
    const credential = randomBytes(32).toString("base64url");
    fs.writeFileSync(path.join(cwd, KEY_NAME), `${credential}\n`, {
      flag: "wx",
      mode: 0o600,
    });
    fs.writeFileSync(configPath, `${JSON.stringify(config, null, 2)}\n`, {
      flag: "wx",
      mode: 0o600,
    });
    fs.mkdirSync(config.stateDirectory, { recursive: true, mode: 0o700 });
    const starterPath = path.join(cwd, CONFIG_FILE_NAME);
    let starterWritten = true;
    try {
      fs.writeFileSync(starterPath, STARTER_CONFIG, {
        flag: "wx",
        mode: 0o600,
      });
    } catch (error) {
      if (!(
        error instanceof Error &&
        "code" in error &&
        error.code === "EEXIST"
      ))
        throw error;
      starterWritten = false;
    }
    process.stdout.write(
      `${starterWritten ? `Wrote starter ${CONFIG_FILE_NAME}\n` : `Kept existing ${CONFIG_FILE_NAME}\n`}Initialized Capstan project ${config.projectId}\nOperator credential: ${path.join(cwd, KEY_NAME)} (0600)\n${credentialIgnored ? "The repository-local Git exclude protects .capstan from ordinary staging." : "Add .capstan/ to .gitignore before staging project files."}\n`,
    );
    return EXIT.ok;
  }
  if (command === "config") {
    const [subcommand, ...extra] = rest;
    if ((subcommand !== "check" && subcommand !== "sync") || extra.length !== 0)
      usage();
    const roleConfig = loadRoleConfig(cwd);
    if (subcommand === "check") {
      process.stdout.write(`${JSON.stringify(roleConfig, null, 2)}\n`);
      return EXIT.ok;
    }
    const { config, credential } = loadConfig(cwd);
    if (
      roleConfig.projectName !== null &&
      roleConfig.projectName.normalize("NFC") !== config.name.normalize("NFC")
    )
      throw new InvalidInputError(
        `${CONFIG_FILE_NAME} project.name does not match the initialized project`,
      );
    if (!fs.existsSync(path.join(config.stateDirectory, "controller.sqlite")))
      throw new BlockedError(
        "controller record does not exist; create it before syncing roles",
      );
    const core = await ControllerCore.open({
      stateDirectory: config.stateDirectory,
      project: project(config, credential, []),
      workspaceRoot: cwd,
    });
    try {
      const result = syncConfiguredRoles(core, roleConfig, () =>
        context(core, credential),
      );
      output({ schemaVersion: 1, ...result }, true);
    } finally {
      core.close();
    }
    return EXIT.ok;
  }
  if (command === "daemon") {
    if (rest.length !== 0) usage();
    const { config, credential } = loadOperator(cwd);
    const stamp = (): string => new Date().toISOString();
    const capstan = fs.existsSync(path.join(cwd, CONFIG_FILE_NAME))
      ? loadRoleConfig(cwd)
      : undefined;
    const adapter =
      capstan === undefined
        ? undefined
        : new HerdrAdapter({
            run: createHerdrRunner({ session: capstan.herdrSession }),
          });
    const notifier =
      capstan === undefined || adapter === undefined
        ? undefined
        : createNotifier({
            adapter,
            channels: capstan.notifications,
            recordPath: path.join(config.stateDirectory, "notifications.jsonl"),
            log: (event, detail) =>
              process.stdout.write(
                `${JSON.stringify({ ts: stamp(), command: `notifier:${event}`, detail })}\n`,
              ),
          });
    try {
      await runDaemon({
        stateDirectory: config.stateDirectory,
        project: project(config, credential, PLACEHOLDER_INPUTS),
        workspaceRoot: cwd,
        log: (entry) =>
          process.stdout.write(
            `${JSON.stringify({ ts: stamp(), ...entry })}\n`,
          ),
        announce: (event) =>
          process.stdout.write(
            `${JSON.stringify({ ts: stamp(), ...event })}\n`,
          ),
        ...(capstan === undefined ? {} : { capstan }),
        ...(adapter === undefined ? {} : { adapter }),
        ...(notifier === undefined ? {} : { notifier }),
      });
    } catch (error) {
      if (error instanceof ControllerOwnershipError)
        throw new BlockedError(error.message);
      throw error;
    } finally {
      adapter?.close();
    }
    return EXIT.ok;
  }
  if (command === "start") {
    const parsed = parseOptions(rest);
    if (parsed.positional.length !== 0) usage();
    const operator = loadOperator(cwd);
    try {
      const result = await ensureDaemon({
        socketPath: operator.socketPath,
        credential: operator.credential,
        projectRoot: cwd,
        logPath: operator.logPath,
        cliPath: fileURLToPath(import.meta.url),
        env: process.env,
      });
      output(
        { running: true, pid: result.pid, started: result.started },
        parsed.json,
      );
    } catch (error) {
      throw controllerUnavailable(error);
    }
    return EXIT.ok;
  }
  if (command === "stop") {
    const parsed = parseOptions(rest);
    if (parsed.positional.length !== 0) usage();
    const operator = loadOperator(cwd);
    try {
      const result = await stopDaemon(operator.socketPath, operator.credential);
      output({ running: false, result }, parsed.json);
    } catch (error) {
      throw controllerUnavailable(error);
    }
    return EXIT.ok;
  }
  if (
    command === "ping" ||
    (command !== undefined && ROUTED_COMMANDS.has(command)) ||
    (command === "pm" &&
      rest.filter((arg) => arg !== "--json")[0] === "restart")
  ) {
    const parsed = parseOptions(rest);
    if (command === "pm") parsed.positional.shift();
    const name = command === "pm" ? "pm-restart" : command;
    if (name === "ping" && parsed.positional.length !== 0) usage();
    return await runRouted(name, parsed.positional, cwd, parsed.json);
  }
  if (command === "run") {
    const briefAt = rest.indexOf("--brief");
    if (briefAt < 0 || !rest[briefAt + 1] || rest.length !== 2) usage();
    const { config, credential } = loadConfig(cwd);
    const briefPath = path.resolve(cwd, rest[briefAt + 1]!);
    let plan: WorkflowPlan;
    try {
      plan = validateWorkflowPlan(readJson(briefPath)).plan;
    } catch (error) {
      if (error instanceof TypeError || error instanceof SyntaxError)
        throw error;
      throw new InvalidInputError(
        error instanceof Error ? error.message : String(error),
      );
    }
    if (
      plan.limits.maxSlices > config.maxSlices ||
      plan.limits.maxRunMs > config.maxRunMs ||
      plan.limits.maxDispatches > config.maxDispatches
    )
      throw new InvalidInputError(
        "brief limits exceed project-local configuration",
      );
    const base = spawnSync(
      "git",
      ["-C", cwd, "rev-parse", "--verify", "HEAD^{commit}"],
      { encoding: "utf8" },
    );
    if (base.status !== 0)
      throw new Error(
        `cannot resolve deterministic project Git base: ${(base.stderr || "git rev-parse failed").trim()}`,
      );
    const baseSha = base.stdout.trim();
    if (!/^[a-f0-9]{40}([a-f0-9]{24})?$/.test(baseSha))
      throw new Error("Git returned an invalid base commit");
    const root = spawnSync("git", ["-C", cwd, "rev-parse", "--show-toplevel"], {
      encoding: "utf8",
    });
    if (
      root.status !== 0 ||
      path.resolve(root.stdout.replace(/\r?\n$/, "")) !== cwd
    )
      throw new InvalidInputError(
        "project directory must be the Git repository root before cloning role workspaces",
      );
    for (const args of [
      [
        "rev-list",
        "--objects",
        "--all",
        "HEAD",
        "--",
        ":(glob)**/.capstan",
        ":(glob)**/.capstan/**",
        ".home/bridge.jsonl",
      ],
      [
        "ls-files",
        "-z",
        "--cached",
        "--",
        ":(glob)**/.capstan",
        ":(glob)**/.capstan/**",
        ".home/bridge.jsonl",
      ],
    ]) {
      const trackedState = spawnSync("git", ["-C", cwd, ...args], {
        encoding: "buffer",
      });
      if (trackedState.status !== 0)
        throw new Error("cannot inspect tracked Capstan state");
      if (trackedState.stdout.length)
        throw new InvalidInputError(
          "project Git history or index contains .capstan state; remove it before cloning role workspaces",
        );
    }
    const initialProject = project(
      config,
      credential,
      projectInputs(config, plan, baseSha),
    );
    const priorDatabase = fs.existsSync(
      path.join(config.stateDirectory, "controller.sqlite"),
    );
    const providerHost = process.env.M1_PROVIDER_HOST;
    if (!providerHost)
      throw new Error(
        "M1_PROVIDER_HOST is required; no runtime host fallback is permitted",
      );
    const workspaceRoot = path.join(cwd, ".capstan", "workspaces");
    fs.mkdirSync(workspaceRoot, { recursive: true, mode: 0o700 });
    const workspaceRootStat = fs.lstatSync(workspaceRoot);
    if (
      !workspaceRootStat.isDirectory() ||
      workspaceRootStat.isSymbolicLink() ||
      (process.getuid && workspaceRootStat.uid !== process.getuid()) ||
      fs.realpathSync(workspaceRoot) !== path.resolve(workspaceRoot)
    )
      throw new Error(
        "role workspace root must be a project-local directory owned by the current user",
      );
    fs.chmodSync(workspaceRoot, 0o700);
    for (const [name, value] of [
      ["M1_HERDR_BINARY", process.env.M1_HERDR_BINARY],
      ["M1_OMP_BINARY", process.env.M1_OMP_BINARY],
      ["M1_OMP_NATIVE_ADDON", process.env.M1_OMP_NATIVE_ADDON],
      ["M1_NODE_BINARY", process.env.M1_NODE_BINARY],
    ] as const) {
      if (!value || !path.isAbsolute(value))
        throw new Error(
          `${name} must be explicitly configured as an absolute runtime binary path`,
        );
    }
    const manager = new RoleRuntimeManager({
      stateRoot: path.join(config.stateDirectory, "runtime"),
      providerHost,
      ...(process.env.M1_PROVIDER_PORT
        ? { providerPort: Number(process.env.M1_PROVIDER_PORT) }
        : {}),
      herdrBinary: process.env.M1_HERDR_BINARY!,
      ompBinary: process.env.M1_OMP_BINARY!,
      ompNativeAddon: process.env.M1_OMP_NATIVE_ADDON!,
      nodeBinary: process.env.M1_NODE_BINARY!,
    });
    let core: ControllerCore;
    let runtimeSocketRoot: string | undefined;
    const allSessions: RoleRuntimeSession[] = [];
    const containedSessions = new Set<string>();
    let removeSignalHandlers = () => {};
    let runtimeManagerClosed = false;
    try {
      core = await ControllerCore.open({
        stateDirectory: config.stateDirectory,
        project: initialProject,
        workspaceRoot: cwd,
        runtimeWorkspacePath: "/workspace",
      });
    } catch (error) {
      await manager.close();
      throw error;
    }
    try {
      const socketRoot = fs.mkdtempSync(path.join(os.tmpdir(), "cs-"));
      runtimeSocketRoot = socketRoot;
      const socketRootStat = fs.lstatSync(socketRoot);
      if (
        !socketRootStat.isDirectory() ||
        socketRootStat.isSymbolicLink() ||
        (process.getuid && socketRootStat.uid !== process.getuid()) ||
        (socketRootStat.mode & 0o077) !== 0 ||
        fs.realpathSync(socketRoot) !== socketRoot
      )
        throw new Error("temporary runtime socket root is not private");
      const recoveredSessions: RoleRuntimeSession[] = [];
      if (priorDatabase) {
        const contextsPath = path.join(
          config.stateDirectory,
          "mutation-contexts.json",
        );
        if (!fs.existsSync(contextsPath))
          throw new BlockedError(
            "restart cannot recover stable scheduler identities because mutation context history is missing",
          );
        core.reconcile();
        const snapshot = core.statusSnapshot();
        if (
          snapshot.run.state !== "active" &&
          snapshot.run.state !== "paused" &&
          snapshot.run.state !== "canceling"
        )
          throw new BlockedError(
            `restart cannot resume a run in ${snapshot.run.state} state`,
          );
        const activeSessions = core
          .listRuntimeSessions()
          .filter((session) => session.state !== "exited");
        const recoveryErrors: string[] = [];
        for (const session of activeSessions) {
          try {
            const metadataPath = path.join(
              config.stateDirectory,
              "runtime",
              "runtime-sessions",
              `${session.sessionId}.json`,
            );
            const metadataDirectory = path.dirname(metadataPath);
            const directoryStat = fs.lstatSync(metadataDirectory);
            if (
              !directoryStat.isDirectory() ||
              directoryStat.isSymbolicLink() ||
              (process.getuid && directoryStat.uid !== process.getuid()) ||
              (directoryStat.mode & 0o077) !== 0 ||
              fs.realpathSync(metadataDirectory) !== metadataDirectory
            )
              throw new Error("runtime metadata directory is not private");
            const metadataStat = fs.lstatSync(metadataPath);
            if (
              !metadataStat.isFile() ||
              metadataStat.isSymbolicLink() ||
              (process.getuid && metadataStat.uid !== process.getuid()) ||
              (metadataStat.mode & 0o077) !== 0
            )
              throw new Error("runtime metadata is not a private regular file");
            const metadata = objectRecord(readJson(metadataPath));
            const recoveredSession = objectRecord(metadata?.session);
            if (recoveredSession?.sessionId !== session.sessionId)
              throw new Error(
                "runtime metadata does not match persisted session identity",
              );
            const attemptedCommand = session.assignmentId
              ? core.attemptedPrestartCommand(session.assignmentId)
              : undefined;
            let prestartSnapshot: BridgeCommandSnapshot | undefined;
            let inspectionFailure: unknown;
            if (attemptedCommand) {
              try {
                if (
                  typeof recoveredSession.bridgeSocketPath !== "string" ||
                  typeof recoveredSession.receiptSocketPath !== "string"
                )
                  throw new Error("persisted bridge endpoints are missing");
                const inspector = new M1BridgeAdapter(
                  core,
                  recoveredSession.receiptSocketPath,
                  recoveredSession.bridgeSocketPath,
                  { allowUnauthenticatedLocalPeers: true },
                );
                prestartSnapshot =
                  await inspector.inspectUncertainCommand(attemptedCommand);
              } catch (error) {
                inspectionFailure = error;
              }
            }
            const proof = await manager.recover(session.sessionId);
            if (
              recoveredSession?.seatId !== session.seatId ||
              (session.containerId &&
                session.containerId !== proof.containerId) ||
              recoveredSession.containerId !== proof.containerId
            )
              throw new Error(
                "runtime recovery proof does not match the persisted session identity",
              );
            if (!session.assignmentId)
              throw new Error(
                "persisted runtime has no assignment association",
              );
            if (inspectionFailure)
              throw new Error(
                `physical runtime was contained, but the prestart bridge snapshot is unavailable: ${String(inspectionFailure)}`,
              );
            if (!core.assignmentIsContained(session.assignmentId))
              core.confirmContainment(
                context(core, credential),
                session.assignmentId,
                JSON.stringify(proof),
                prestartSnapshot,
              );
            recoveredSessions.push(
              recoveredSession as unknown as RoleRuntimeSession,
            );
            const current = core
              .listRuntimeSessions()
              .find((entry) => entry.sessionId === session.sessionId);
            if (current?.state === "ready" || current?.state === "starting")
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "unknown",
              );
            if (
              current?.state === "ready" ||
              current?.state === "working" ||
              current?.state === "starting" ||
              current?.state === "unknown"
            )
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "stopping",
              );
            if (
              current?.state === "ready" ||
              current?.state === "working" ||
              current?.state === "starting" ||
              current?.state === "unknown" ||
              current?.state === "stopping"
            )
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "exited",
              );
          } catch (error) {
            recoveryErrors.push(`${session.sessionId}: ${String(error)}`);
          }
        }
        const registeredSessions = new Set(
          core.listRuntimeSessions().map((session) => session.sessionId),
        );
        let persistedSessionIds: readonly string[] = [];
        try {
          persistedSessionIds = manager.listPersistedSessionIds();
        } catch (error) {
          recoveryErrors.push(
            `persisted runtime sessions could not be listed: ${String(error)}`,
          );
        }
        for (const sessionId of persistedSessionIds) {
          if (registeredSessions.has(sessionId)) continue;
          try {
            const proof = await manager.recover(sessionId);
            if (
              !proof.containerAbsent ||
              !proof.cgroupEmpty ||
              !proof.egressPolicyRemoved ||
              !proof.networkRemoved
            )
              throw new Error("orphan runtime containment was not proven");
          } catch (error) {
            recoveryErrors.push(
              `${sessionId}: unregistered runtime containment failed: ${String(error)}`,
            );
          }
        }
        if (recoveryErrors.length)
          throw new BlockedError(
            `restart recovery could not prove containment; run remains visibly blocked: ${recoveryErrors.join("; ")}`,
          );
        for (const session of recoveredSessions) {
          allSessions.push(session);
          containedSessions.add(session.sessionId);
        }
        if (snapshot.run.state === "canceling") {
          if (core.hasUncontainedAssignmentAuthority())
            throw new BlockedError(
              "restart cannot finish cancellation while assignment authority remains uncontained",
            );
          core.transitionRun(context(core, credential), "canceled");
          throw new BlockedError(
            "prior cancellation is now contained and terminal; this run will not resume dispatch",
          );
        }
      }
      const contexts = new FileMutationContextStore(
        path.join(config.stateDirectory, "mutation-contexts.json"),
        credential,
        validateWorkflowPlan(plan).hash,
      );
      const roleNames = ["PM", "Developer", "Verifier", "Supervisor"] as const;
      const persistedRoles = priorDatabase ? core.statusSnapshot().roles : [];
      const seatId = (role: (typeof roleNames)[number]) => {
        const existing = persistedRoles.find((entry) => entry.role === role);
        if (priorDatabase && !existing)
          throw new BlockedError(
            `restart cannot reconstruct the persisted ${role} seat`,
          );
        return existing?.seatId ?? `${role.toLowerCase()}-${randomUUID()}`;
      };
      const seats = {
        PM: { seatId: seatId("PM"), name: "PM", displayName: "PM" },
        Developer: {
          seatId: seatId("Developer"),
          name: "Developer",
          displayName: "Developer",
        },
        Verifier: {
          seatId: seatId("Verifier"),
          name: "Verifier",
          displayName: "Verifier",
        },
        Supervisor: {
          seatId: seatId("Supervisor"),
          name: "Supervisor",
          displayName: "Supervisor",
        },
      };
      const adapters: Record<string, M1BridgeAdapter> = {};
      const peerVerifiers: Record<
        string,
        { current?: (socketFd: number) => boolean }
      > = {};
      const sessions: Record<string, RoleRuntimeSession> = {};
      const proven: RoleRuntimeContainmentProof[] = [];
      const workspaceMetadata: Record<
        string,
        { workspace: string; baseSha: string }
      > = {};
      const candidateWorkspaces: Record<string, string> = {};
      const acceptedCandidateByWorkItem = new Map<string, string>();
      const candidateUnderReviewByWorkItem = new Map<string, string>();
      let finalVerificationSource:
        { workspace: string; commitSha: string } | undefined;
      const runtimeAssignments: Record<string, string> = {};
      const runtimeCommands: Record<string, string> = {};
      const uncertainCommandSnapshots = new Map<
        string,
        BridgeCommandSnapshot
      >();
      let provisionFailureUnproven = false;
      let dispatches = 0;
      const persistedSessionCount = priorDatabase
        ? core.listRuntimeSessions().length
        : 0;
      const progressedWorkCount = priorDatabase
        ? core
            .statusSnapshot()
            .work.filter(
              (work) => !["pending", "ready", "blocked"].includes(work.state),
            ).length
        : 0;
      dispatches = priorDatabase
        ? Math.min(
            plan.limits.maxDispatches,
            Math.max(persistedSessionCount, progressedWorkCount),
          )
        : 0;
      for (const session of recoveredSessions) {
        sessions[session.seatId] = session;
        const persisted = core
          .listRuntimeSessions()
          .find((entry) => entry.sessionId === session.sessionId);
        if (!persisted?.assignmentId) continue;
        runtimeAssignments[session.sessionId] = persisted.assignmentId;
        if (
          !["PM", "Developer", "Verifier", "Supervisor"].includes(session.role)
        )
          continue;
        const assignment = core.latestAssignmentForRole(
          session.role as "PM" | "Developer" | "Verifier" | "Supervisor",
          persisted.assignmentId,
        );
        const report = assignment
          ? core.latestCompletedReport(
              assignment.workItemId,
              persisted.assignmentId,
            )
          : undefined;
        if (!assignment)
          throw new BlockedError(
            `persisted runtime has no matching assignment ${persisted.assignmentId}`,
          );
        if (
          assignment.seatId !== session.seatId ||
          path.resolve(session.workspace) !==
            path.resolve(
              workspaceRoot,
              assignment.seatId,
              assignment.workItemId,
              String(assignment.generation),
            )
        )
          throw new BlockedError(
            `persisted runtime workspace does not match assignment ${assignment.assignmentId}`,
          );
        if (report && session.role === "Developer") {
          const reported = objectRecord(
            parseJsonWithoutDuplicateMembers(report.reply),
          );
          if (
            typeof reported?.baseSha !== "string" ||
            !/^[a-f0-9]{40}([a-f0-9]{24})?$/.test(reported.baseSha)
          )
            throw new BlockedError(
              `reported Developer base is unavailable for ${assignment.assignmentId}`,
            );
          workspaceMetadata[session.sessionId] = {
            workspace: session.workspace,
            baseSha: reported.baseSha,
          };
        }
      }
      const verifiedGitEnv = sanitizedGitEnvironment();
      if (priorDatabase) {
        const snapshot = core.statusSnapshot();
        for (const evidence of snapshot.evidence) {
          const candidate = objectRecord(
            objectRecord(core.inspect(evidence.candidateId))?.record,
          );
          const assignmentId = candidate?.assignment_id;
          if (typeof assignmentId !== "string") continue;
          const assignment = core.latestAssignmentForRole(
            "Developer",
            assignmentId,
          );
          if (!assignment) continue;
          const workspace = path.join(
            workspaceRoot,
            assignment.seatId,
            assignment.workItemId,
            String(assignment.generation),
          );
          try {
            assertTrackedCheckoutMatchesHead(workspace, evidence.commitSha);
            candidateWorkspaces[evidence.candidateId] = workspace;
          } catch {
            continue;
          }
          const work = snapshot.work.find(
            (entry) => entry.workItemId === assignment.workItemId,
          );
          if (work?.state === "awaiting_verification")
            candidateUnderReviewByWorkItem.set(
              work.workItemId,
              evidence.candidateId,
            );
        }
        for (const work of snapshot.work) {
          if (work.state !== "accepted") continue;
          const record = objectRecord(
            objectRecord(core.inspect(work.workItemId))?.record,
          );
          const candidateId = record?.accepted_candidate_id;
          if (
            typeof candidateId === "string" &&
            candidateWorkspaces[candidateId]
          )
            acceptedCandidateByWorkItem.set(work.workItemId, candidateId);
        }
      }
      let closeControl: (() => Promise<void>) | undefined;
      let stopping = false;
      let paused =
        priorDatabase && core.statusSnapshot().run.state === "paused";
      const priorPmAssignment = priorDatabase
        ? core.latestAssignmentForRole("PM")
        : undefined;
      let pmAssignmentId = priorPmAssignment?.assignmentId ?? "";
      let pmWorkItemId = priorPmAssignment?.workItemId ?? "";
      const signal = () => {
        stopping = true;
      };
      const waitForDispatchPermission = async () => {
        while (paused && !stopping && Date.now() < deadlineMs) {
          const { promise, resolve } = Promise.withResolvers<void>();
          setTimeout(resolve, 100);
          await promise;
        }
        if (stopping) throw new Error("run canceled before dispatch");
        if (Date.now() >= deadlineMs)
          throw new Error("maxRunMs exceeded before dispatch");
      };
      const pendingProvisions = new Set<Promise<unknown>>();
      const provisionRuntime = async (
        role: Exclude<Role, "operator" | "controller">,
        seatId: string,
        workItemId: string,
        generation: number,
        assignmentId: string,
        predecessorCandidates: readonly {
          workspace: string;
          commitSha: string;
        }[] = [],
        exactCandidate?: { workspace: string; commitSha: string },
      ): Promise<{
        session: RoleRuntimeSession;
        adapter: M1BridgeAdapter;
        baseSha: string;
        evidenceDirectory?: string;
      }> => {
        await waitForDispatchPermission();
        const workspace = path.join(
          workspaceRoot,
          seatId,
          workItemId,
          String(generation),
        );
        fs.mkdirSync(path.dirname(workspace), { recursive: true, mode: 0o700 });
        for (const directory of [
          path.join(workspaceRoot, seatId),
          path.join(workspaceRoot, seatId, workItemId),
        ]) {
          const stat = fs.lstatSync(directory);
          if (
            !stat.isDirectory() ||
            stat.isSymbolicLink() ||
            (process.getuid && stat.uid !== process.getuid()) ||
            !fs
              .realpathSync(directory)
              .startsWith(`${workspaceRoot}${path.sep}`)
          )
            throw new Error(
              "assignment workspace parent escaped the project-local workspace root",
            );
          fs.chmodSync(directory, 0o700);
        }
        const hostGitArgs = [
          "--no-replace-objects",
          "-c",
          "core.fsmonitor=false",
          "-c",
          "core.hooksPath=/dev/null",
        ];
        const hostGitEnv = {
          ...verifiedGitEnv,
          GIT_CONFIG_COUNT: "1",
          GIT_CONFIG_KEY_0: "uploadpack.packObjectsHook",
          GIT_CONFIG_VALUE_0: "/bin/true",
        };
        const runHostGit = (args: string[]) => {
          const timeout = deadlineMs - Date.now();
          if (timeout <= 0)
            throw new BlockedError(
              "maxRunMs exceeded during role workspace provisioning",
            );
          return spawnSync("git", args, {
            encoding: "utf8",
            env: hostGitEnv,
            timeout,
          });
        };
        const cloned = runHostGit([
          ...hostGitArgs,
          "clone",
          "--no-checkout",
          "--no-local",
          "--quiet",
          "--",
          cwd,
          workspace,
        ]);
        if (cloned.status !== 0)
          throw new Error(
            `cannot create isolated ${role} assignment checkout: ${(cloned.stderr || "git clone failed").trim()}`,
          );
        fs.chmodSync(workspace, 0o700);
        if (role === "Developer" || predecessorCandidates.length > 0) {
          for (const key of ["user.name", "user.email"] as const) {
            const sourceIdentity = runHostGit([
              "-C",
              cwd,
              "config",
              "--get",
              key,
            ]);
            const value =
              sourceIdentity.status === 0 ? sourceIdentity.stdout.trim() : "";
            if (!value)
              throw new Error(
                `Developer candidate commits require project Git ${key}`,
              );
            const isolatedIdentity = runHostGit([
              "-C",
              workspace,
              "config",
              "--local",
              key,
              value,
            ]);
            if (isolatedIdentity.status !== 0)
              throw new Error(
                `cannot configure ${key} in isolated Developer checkout: ${(isolatedIdentity.stderr || "git config failed").trim()}`,
              );
          }
        }
        if (exactCandidate) {
          const fetched = runHostGit([
            ...hostGitArgs,
            "-C",
            workspace,
            "fetch",
            "--quiet",
            "--no-tags",
            exactCandidate.workspace,
            exactCandidate.commitSha,
          ]);
          if (fetched.status !== 0)
            throw new Error(
              `cannot fetch exact candidate ${exactCandidate.commitSha}: ${(fetched.stderr || "git fetch failed").trim()}`,
            );
          const candidateCheckout = runHostGit([
            ...hostGitArgs,
            "-C",
            workspace,
            "checkout",
            "--quiet",
            "--detach",
            "FETCH_HEAD",
          ]);
          if (candidateCheckout.status !== 0)
            throw new Error(
              `cannot check out exact candidate ${exactCandidate.commitSha}: ${(candidateCheckout.stderr || "git checkout failed").trim()}`,
            );
        } else {
          const checkedOut = runHostGit([
            ...hostGitArgs,
            "-C",
            workspace,
            "checkout",
            "--quiet",
            "--detach",
            baseSha,
          ]);
          if (checkedOut.status !== 0)
            throw new Error(
              `cannot check out accepted base ${baseSha}: ${(checkedOut.stderr || "git checkout failed").trim()}`,
            );
          for (const predecessor of predecessorCandidates) {
            const fetched = runHostGit([
              ...hostGitArgs,
              "-C",
              workspace,
              "fetch",
              "--quiet",
              "--no-tags",
              predecessor.workspace,
              predecessor.commitSha,
            ]);
            if (fetched.status !== 0)
              throw new Error(
                `cannot fetch accepted predecessor ${predecessor.commitSha}: ${(fetched.stderr || "git fetch failed").trim()}`,
              );
            const merged = runHostGit([
              ...hostGitArgs,
              "-C",
              workspace,
              "merge",
              "--quiet",
              "--no-edit",
              "--no-ff",
              "FETCH_HEAD",
            ]);
            if (merged.status !== 0)
              throw new Error(
                `accepted predecessor ${predecessor.commitSha} does not merge cleanly: ${(merged.stderr || "git merge failed").trim()}`,
              );
          }
        }
        const actualBase = runHostGit(["-C", workspace, "rev-parse", "HEAD"]);
        if (
          actualBase.status !== 0 ||
          !/^[a-f0-9]{40}([a-f0-9]{24})?$/.test(actualBase.stdout.trim())
        )
          throw new Error("assignment workspace has no valid committed base");
        if (
          exactCandidate &&
          actualBase.stdout.trim().toLowerCase() !==
            exactCandidate.commitSha.toLowerCase()
        )
          throw new Error(
            "assignment checkout HEAD does not equal its bound commit",
          );
        const homeDirectory = path.join(workspace, ".home");
        fs.mkdirSync(homeDirectory, { recursive: true, mode: 0o700 });
        const homeStat = fs.lstatSync(homeDirectory);
        if (
          !homeStat.isDirectory() ||
          homeStat.isSymbolicLink() ||
          (process.getuid && homeStat.uid !== process.getuid()) ||
          fs.realpathSync(homeDirectory) !== homeDirectory
        )
          throw new Error("isolated checkout home is not a private directory");
        fs.chmodSync(homeDirectory, 0o700);
        const journalMountpoint = path.join(homeDirectory, "bridge.jsonl");
        try {
          fs.writeFileSync(journalMountpoint, "", {
            flag: "wx",
            mode: 0o600,
          });
        } catch (error) {
          if (
            !(error instanceof Error) ||
            !("code" in error) ||
            error.code !== "EEXIST"
          )
            throw error;
        }
        const journalMountStat = fs.lstatSync(journalMountpoint);
        if (
          !journalMountStat.isFile() ||
          journalMountStat.isSymbolicLink() ||
          (process.getuid && journalMountStat.uid !== process.getuid())
        )
          throw new Error("isolated checkout journal mountpoint is not a file");
        fs.chmodSync(journalMountpoint, 0o600);
        const excludePath = path.join(workspace, ".git", "info", "exclude");
        const excludeStat = fs.lstatSync(excludePath);
        if (!excludeStat.isFile() || excludeStat.isSymbolicLink())
          throw new Error(
            "isolated checkout Git exclude is not a regular file",
          );
        const excludeContents = fs.readFileSync(excludePath, "utf8");
        if (!/^\.home\/?$/m.test(excludeContents))
          fs.appendFileSync(
            excludePath,
            `${excludeContents.endsWith("\n") || excludeContents.length === 0 ? "" : "\n"}.home/\n`,
            { mode: 0o600 },
          );
        const bridgeDir = path.join(socketRoot, "bridge", seatId);
        const receiptDir = path.join(socketRoot, "receipt", seatId);
        const journalDir = path.join(config.stateDirectory, "journals");
        for (const directory of [bridgeDir, receiptDir, journalDir]) {
          fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
          fs.chmodSync(directory, 0o700);
        }
        const bridgePath = path.join(bridgeDir, "seat.sock");
        const receiptPath = path.join(receiptDir, "receipt.sock");
        if (!adapters[seatId]) {
          const journalPath = path.join(
            journalDir,
            `${role.toLowerCase()}.jsonl`,
          );
          try {
            fs.writeFileSync(journalPath, "", { flag: "wx", mode: 0o600 });
          } catch (error) {
            if (
              !(error instanceof Error) ||
              !("code" in error) ||
              error.code !== "EEXIST"
            )
              throw error;
            const journal = fs.lstatSync(journalPath);
            if (
              !journal.isFile() ||
              journal.isSymbolicLink() ||
              (journal.mode & 0o077) !== 0 ||
              (process.getuid && journal.uid !== process.getuid())
            )
              throw new Error(
                `role journal is not a private regular file: ${journalPath}`,
              );
          }
          peerVerifiers[seatId] = {};
          const adapter = new M1BridgeAdapter(
            core,
            receiptPath,
            bridgePath,
            {
              authenticate: (socketFd) =>
                peerVerifiers[seatId]?.current?.(socketFd) ?? false,
            },
            { path: journalPath, role },
          );
          await adapter.listen();
          adapters[seatId] = adapter;
        }
        let evidenceDirectory: string | undefined;
        if (role === "Verifier" || role === "Supervisor") {
          const evidenceRoot = path.join(
            config.stateDirectory,
            "runtime",
            "evidence",
          );
          fs.mkdirSync(evidenceRoot, { recursive: true, mode: 0o700 });
          fs.chmodSync(evidenceRoot, 0o700);
          if (role === "Supervisor") evidenceDirectory = evidenceRoot;
          else {
            const evidenceParent = path.join(evidenceRoot, seatId, workItemId);
            fs.mkdirSync(evidenceParent, { recursive: true, mode: 0o700 });
            fs.chmodSync(evidenceParent, 0o700);
            evidenceDirectory = path.join(evidenceParent, String(generation));
            fs.mkdirSync(evidenceDirectory, { mode: 0o700 });
            fs.chmodSync(evidenceDirectory, 0o700);
          }
        }
        if (stopping)
          throw new Error("run canceled before runtime provisioning");
        const pending = manager.provision(role, seatId, workspace, {
          journalPath: path.join(journalDir, `${role.toLowerCase()}.jsonl`),
          receiptSocketPath: receiptPath,
          bridgeSocketPath: bridgePath,
          ...(evidenceDirectory ? { evidenceDirectory } : {}),
        });
        const tracked = pending.then(
          (session) => {
            sessions[seatId] = session;
            runtimeAssignments[session.sessionId] = assignmentId;
            allSessions.push(session);
            return undefined;
          },
          (error: unknown) => {
            provisionFailureUnproven = true;
            return error;
          },
        );
        pendingProvisions.add(tracked);
        let session: RoleRuntimeSession;
        try {
          session = await pending;
        } finally {
          pendingProvisions.delete(tracked);
        }
        if (stopping)
          throw new Error("run canceled while a role runtime was provisioning");
        peerVerifiers[seatId]!.current = expectedReceiptPeer(
          session.helperPath,
          session.expectedOmpHostPid,
        );
        core.createRuntimeSession(context(core, credential), {
          sessionId: session.sessionId,
          seatId,
          assignmentId,
          provider: "m1",
          profile: session.profile,
          workspace,
        });
        core.recordUsage(context(core, credential), {
          observationId: randomUUID(),
          sessionId: session.sessionId,
          assignmentId,
          provider: "m1",
          metric: "runtime_usage",
          availability: "unavailable",
          detail: { reason: "provider usage accounting is unavailable" },
        });
        core.recordRuntimeIdentity(
          context(core, credential),
          session.sessionId,
          {
            observedPid: session.expectedOmpHostPid,
            containerId: session.containerId,
            endpoint: session.bridgeSocketPath,
          },
        );
        core.transitionRuntimeSession(
          context(core, credential),
          session.sessionId,
          "ready",
        );
        workspaceMetadata[session.sessionId] = {
          workspace,
          baseSha: actualBase.stdout.trim(),
        };
        return {
          session,
          adapter: adapters[seatId]!,
          baseSha: actualBase.stdout.trim(),
          ...(evidenceDirectory ? { evidenceDirectory } : {}),
        };
      };
      const containRuntimeOnce = async (
        session: RoleRuntimeSession,
        assignmentId: string,
      ) => {
        const runtime = core
          .listRuntimeSessions()
          .find((entry) => entry.sessionId === session.sessionId);
        if (runtime?.state === "ready" || runtime?.state === "starting")
          core.transitionRuntimeSession(
            context(core, credential),
            session.sessionId,
            "unknown",
          );
        if (
          runtime?.state === "ready" ||
          runtime?.state === "starting" ||
          runtime?.state === "working" ||
          runtime?.state === "unknown"
        )
          core.transitionRuntimeSession(
            context(core, credential),
            session.sessionId,
            "stopping",
          );
        const proof = await manager!.stopAndContain(
          session.role,
          session.sessionId,
        );
        core.confirmContainment(
          context(core, credential),
          assignmentId,
          JSON.stringify(proof),
        );
        if (runtime)
          core.transitionRuntimeSession(
            context(core, credential),
            session.sessionId,
            "exited",
          );
        containedSessions.add(session.sessionId);
        return proof;
      };
      const inFlightContainments = new Map<
        string,
        ReturnType<typeof containRuntimeOnce>
      >();
      const containRuntime = (
        session: RoleRuntimeSession,
        assignmentId: string,
      ) => {
        const running = inFlightContainments.get(session.sessionId);
        if (running) return running;
        const attempt = containRuntimeOnce(session, assignmentId).finally(() =>
          inFlightContainments.delete(session.sessionId),
        );
        inFlightContainments.set(session.sessionId, attempt);
        return attempt;
      };
      const inspectUncertainCommands = async (skipContained = false) => {
        for (const session of allSessions) {
          const commandId = runtimeCommands[session.sessionId];
          if (!commandId || uncertainCommandSnapshots.has(session.sessionId))
            continue;
          if (skipContained && containedSessions.has(session.sessionId))
            continue;
          const state = core.commandState(commandId);
          if (state !== "attempting" && state !== "unknown") continue;
          const adapter = adapters[session.seatId];
          if (!adapter)
            throw new Error(
              `missing bridge adapter for uncertain command ${commandId}`,
            );
          uncertainCommandSnapshots.set(
            session.sessionId,
            await adapter.inspectUncertainCommand(commandId),
          );
        }
      };
      const reconcileInFlightCommands = async () => {
        if (
          ![...uncertainCommandSnapshots.keys()].some((sessionId) => {
            const commandId = runtimeCommands[sessionId];
            return commandId && core.commandState(commandId) === "attempting";
          })
        )
          return;
        const closeServer = closeControl;
        closeControl = undefined;
        await closeServer?.();
        core.close();
        core = await ControllerCore.open({
          stateDirectory: config.stateDirectory,
          project: initialProject,
          workspaceRoot: cwd,
          runtimeWorkspacePath: "/workspace",
        });
      };
      const containPendingSessions = async (cleanupErrors: unknown[]) => {
        try {
          await inspectUncertainCommands();
        } catch (error) {
          cleanupErrors.push(error);
        }
        for (const session of allSessions) {
          if (containedSessions.has(session.sessionId)) continue;
          try {
            const runtime = core
              .listRuntimeSessions()
              .find((entry) => entry.sessionId === session.sessionId);
            if (runtime?.state === "ready")
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "unknown",
              );
            if (
              runtime?.state === "ready" ||
              runtime?.state === "working" ||
              runtime?.state === "unknown"
            )
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "stopping",
              );
          } catch (error) {
            cleanupErrors.push(error);
          }
          try {
            const proof = await manager!.stopAndContain(
              session.role,
              session.sessionId,
            );
            if (!proven.some((entry) => entry.sessionId === proof.sessionId))
              proven.push(proof);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        try {
          for (const proof of await manager!.close())
            if (!proven.some((entry) => entry.sessionId === proof.sessionId))
              proven.push(proof);
        } catch (error) {
          cleanupErrors.push(error);
        }
        if (
          allSessions.every(
            (session) =>
              containedSessions.has(session.sessionId) ||
              proven.some((proof) => proof.sessionId === session.sessionId),
          )
        ) {
          try {
            await reconcileInFlightCommands();
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        for (const proof of proven) {
          if (containedSessions.has(proof.sessionId)) continue;
          const session = allSessions.find(
            (entry) => entry.sessionId === proof.sessionId,
          );
          const assignmentId = runtimeAssignments[proof.sessionId];
          if (!session || !assignmentId) {
            cleanupErrors.push(
              new Error(
                `runtime manager returned an unknown containment proof for ${proof.sessionId}`,
              ),
            );
            continue;
          }
          try {
            core.confirmContainment(
              context(core, credential),
              assignmentId,
              JSON.stringify(proof),
              uncertainCommandSnapshots.get(proof.sessionId),
            );
            const runtime = core
              .statusSnapshot()
              .roles.find((entry) => entry.seatId === session.seatId);
            if (runtime?.sessionState === "stopping")
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "exited",
              );
            containedSessions.add(session.sessionId);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
      };
      const deadlineMs = contexts.startedAtMs + plan.limits.maxRunMs;
      const waitForReport = async (
        workItemId: string,
        assignmentId: string,
        untilMs = deadlineMs,
      ) => {
        while (!stopping && Date.now() < untilMs) {
          const report = core.latestCompletedReport(workItemId, assignmentId);
          if (report) return report;
          const delay = Promise.withResolvers<void>();
          setTimeout(delay.resolve, Math.min(250, untilMs - Date.now()));
          await delay.promise;
        }
        return undefined;
      };
      const verifiedEvidencePath = (
        evidenceDirectory: string | undefined,
        artifactRef: string,
      ): string => {
        if (!evidenceDirectory || !artifactRef.startsWith("/evidence/"))
          throw new Error(
            "Verifier artifactRef requires a dedicated /evidence mount",
          );
        const evidenceRelative = artifactRef.slice("/evidence/".length);
        if (
          !evidenceRelative ||
          path.isAbsolute(evidenceRelative) ||
          evidenceRelative.split(/[\\/]/).includes("..")
        )
          throw new Error(
            "Verifier artifactRef escapes its dedicated evidence mount",
          );
        const artifactPath = path.resolve(evidenceDirectory, evidenceRelative);
        const artifactRelative = path.relative(evidenceDirectory, artifactPath);
        const artifactStat = fs.lstatSync(artifactPath);
        const artifactRealPath = fs.realpathSync(artifactPath);
        const evidenceRealPath = fs.realpathSync(evidenceDirectory);
        if (
          !artifactRelative ||
          artifactRelative === ".." ||
          artifactRelative.startsWith(`..${path.sep}`) ||
          path.isAbsolute(artifactRelative) ||
          !artifactStat.isFile() ||
          artifactStat.isSymbolicLink() ||
          artifactStat.size === 0 ||
          !artifactRealPath.startsWith(`${evidenceRealPath}${path.sep}`)
        )
          throw new Error(
            "Verifier artifact is not a non-empty regular file in its private evidence directory",
          );
        return artifactRealPath;
      };
      process.on("SIGINT", signal);
      process.on("SIGTERM", signal);
      removeSignalHandlers = () => {
        process.off("SIGINT", signal);
        process.off("SIGTERM", signal);
      };
      let cleanupComplete = false;
      const cancellations = new Set<Promise<void>>();
      let operatorCancellationIncomplete = false;
      let pauseRequests = 0;
      try {
        closeControl = await listenControl(
          path.join(config.stateDirectory, "control.sock"),
          credential,
          core,
          async (action) => {
            const operator = context(core, credential);
            if (action === "pause") {
              const state = core.statusSnapshot().run.state;
              if (state === "active") core.transitionRun(operator, "paused");
              else if (state !== "paused")
                throw new BlockedError(
                  "only an active or paused run can pause",
                );
              pauseRequests++;
              paused = true;
              return core.statusSnapshot().run;
            }
            if (action === "resume") {
              if (core.statusSnapshot().run.state !== "paused")
                throw new BlockedError("only a paused run can resume");
              const pauseRequestsBefore = pauseRequests;
              await inspectUncertainCommands(true);
              if (pauseRequests !== pauseRequestsBefore)
                throw new BlockedError(
                  "a pause was requested while resume was reconciling; the run stays paused",
                );
              if (
                core
                  .listRuntimeSessions()
                  .some((session) => session.state === "unknown") ||
                Object.entries(runtimeCommands).some(
                  ([sessionId, commandId]) =>
                    !containedSessions.has(sessionId) &&
                    ["unknown", "attempting"].includes(
                      core.commandState(commandId) ?? "",
                    ),
                )
              )
                throw new BlockedError(
                  "live resume requires operator reconciliation of uncertain runtime or command authority",
                );
              core.transitionRun(operator, "active");
              paused = false;
              return core.statusSnapshot().run;
            }
            if (cancellations.size)
              throw new BlockedError("cancellation is already in progress");
            const runStateBeforeCancel = core.statusSnapshot().run.state;
            if (runStateBeforeCancel === "canceled")
              return core.statusSnapshot().run;
            if (
              runStateBeforeCancel === "completed" ||
              runStateBeforeCancel === "failed"
            )
              throw new BlockedError(
                `a ${runStateBeforeCancel} run cannot be canceled`,
              );
            const settled = Promise.withResolvers<void>();
            cancellations.add(settled.promise);
            try {
              stopping = true;
              operatorCancellationIncomplete = true;
              paused = false;
              const state = core.statusSnapshot().run.state;
              if (state === "active" || state === "paused")
                core.transitionRun(operator, "canceling");
              core.revokeActiveAssignments(
                context(core, credential),
                "operator requested cancellation",
              );
              const pendingResults = await Promise.all([...pendingProvisions]);
              const failures = pendingResults
                .filter((result) => result !== undefined)
                .map(
                  (result) => `runtime provisioning failed: ${String(result)}`,
                );
              if (provisionFailureUnproven && failures.length === 0)
                failures.push(
                  "runtime provisioning failed without a containment proof",
                );
              for (const session of allSessions) {
                if (containedSessions.has(session.sessionId)) continue;
                try {
                  await containRuntime(
                    session,
                    runtimeAssignments[session.sessionId]!,
                  );
                } catch (error) {
                  failures.push(`${session.sessionId}: ${String(error)}`);
                }
              }
              if (failures.length)
                throw new Error(
                  `cancellation containment incomplete; run remains canceling: ${failures.join("; ")}`,
                );
              core.transitionRun(context(core, credential), "canceled");
              operatorCancellationIncomplete = false;
              return core.statusSnapshot().run;
            } finally {
              settled.resolve();
              cancellations.delete(settled.promise);
            }
          },
        );
        await waitForDispatchPermission();
        const scheduler = new WorkflowScheduler({
          plan: validateWorkflowPlan(plan),
          core,
          operatorCredential: credential,
          startedAtMs: contexts.startedAtMs,
          mutationContexts: contexts,
          seats,
          isStopping: () => stopping,
          isPlanAccepted: ({ planHash }) => {
            const pmWork = core
              .statusSnapshot()
              .work.find((work) => work.workItemId === pmWorkItemId);
            if (!pmWork || pmWork.state !== "accepted") return false;
            const report = core.latestCompletedReport(
              pmWorkItemId,
              pmAssignmentId,
            );
            if (
              !report ||
              report.role !== "PM" ||
              report.authorityState !== "contained" ||
              report.inputRevision !== core.inputRevision
            )
              return false;
            try {
              const response: unknown = parseJsonWithoutDuplicateMembers(
                report.reply,
              );
              return (
                response !== null &&
                typeof response === "object" &&
                "planHash" in response &&
                response.planHash === planHash
              );
            } catch (error) {
              if (error instanceof SyntaxError) return false;
              throw error;
            }
          },
          dispatch: async ({ slice, assignment, getMutationContext }) => {
            if (stopping)
              throw new Error("run canceled before Developer dispatch");
            if (dispatches >= plan.limits.maxDispatches)
              throw new Error(
                "maxDispatches exhausted before Developer dispatch",
              );
            dispatches += 1;
            const predecessorCandidates: {
              workspace: string;
              commitSha: string;
            }[] = [];
            for (const dependency of slice.dependsOn) {
              const dependencyId = workflowWorkItemId(plan.taskId, dependency);
              const dependencyWork = objectRecord(
                objectRecord(core.inspect(dependencyId))?.record,
              );
              const candidateId = dependencyWork?.accepted_candidate_id;
              if (typeof candidateId !== "string")
                throw new Error(
                  `accepted predecessor ${dependency} has no core-accepted candidate`,
                );
              const candidateRecord = objectRecord(
                objectRecord(core.inspect(candidateId))?.record,
              );
              const sourceWorkspace = candidateWorkspaces[candidateId];
              if (
                !sourceWorkspace ||
                typeof candidateRecord?.commit_sha !== "string"
              )
                throw new Error(
                  `accepted predecessor candidate ${candidateId} has no verified workspace source`,
                );
              predecessorCandidates.push({
                workspace: sourceWorkspace,
                commitSha: candidateRecord.commit_sha,
              });
            }
            const { session, adapter } = await provisionRuntime(
              "Developer",
              assignment.seatId,
              assignment.workItemId,
              assignment.generation,
              assignment.assignmentId,
              predecessorCandidates,
            );
            runtimeCommands[session.sessionId] = assignment.commandId;
            if (stopping || Date.now() >= deadlineMs)
              throw new Error(
                stopping
                  ? "run canceled before Developer dispatch"
                  : "maxRunMs exceeded before Developer dispatch",
              );
            await waitForDispatchPermission();
            core.transitionRuntimeSession(
              context(core, credential),
              session.sessionId,
              "working",
            );
            await adapter.dispatchAndStart(
              getMutationContext(),
              assignment.commandId,
            );
          },
        });
        const identities = scheduler.initialize();
        if (!core.statusSnapshot().supervision.enabled)
          core.enableSupervision(context(core, credential));
        if (Date.now() >= deadlineMs)
          throw new Error("maxRunMs exceeded before PM assignment");
        let blocker: string | undefined;
        let supervisorMustPause = false;
        const pmTitle = `Review and accept plan ${plan.taskId}`;
        let pmWork = core
          .statusSnapshot()
          .work.find((work) => work.role === "PM" && work.title === pmTitle);
        if (priorPmAssignment) {
          pmWorkItemId = priorPmAssignment.workItemId;
          if (!pmWork || pmWork.workItemId !== pmWorkItemId)
            throw new BlockedError(
              "restart PM assignment does not match the validated plan work item",
            );
        } else {
          pmWorkItemId = pmWork?.workItemId ?? `pm-${randomUUID()}`;
          if (!pmWork) {
            core.createWorkItem(context(core, credential), {
              workItemId: pmWorkItemId,
              title: pmTitle,
              description: `Review the already validated plan without changing it. Return JSON {"planHash":"..."} with planHash exactly equal to ${validateWorkflowPlan(plan).hash} to approve it. Any other result leaves the plan unaccepted. The acceptance criteria are: ${plan.acceptanceCriteria.join("; ")}`,
              requiredRole: "PM",
            });
            pmWork = core
              .statusSnapshot()
              .work.find((work) => work.workItemId === pmWorkItemId);
            for (const slice of plan.slices)
              core.addDependency(
                context(core, credential),
                workflowWorkItemId(plan.taskId, slice.id),
                pmWorkItemId,
              );
          }
        }
        if (pmWork?.state !== "accepted") {
          for (const slice of plan.slices) {
            const workItemId = workflowWorkItemId(plan.taskId, slice.id);
            const hasPlanGate = core
              .readiness(workItemId)
              .reasons.some((reason) =>
                reason.startsWith(`dependency ${pmWorkItemId} `),
              );
            if (!hasPlanGate)
              core.addDependency(
                context(core, credential),
                workItemId,
                pmWorkItemId,
              );
          }
        }
        let pmReport: CompletedWorkReport | undefined;
        if (
          priorPmAssignment &&
          (pmWork?.state === "accepted" ||
            pmWork?.state === "awaiting_verification")
        ) {
          pmAssignmentId = priorPmAssignment.assignmentId;
          pmReport = core.latestCompletedReport(pmWorkItemId, pmAssignmentId);
          if (
            !pmReport ||
            pmReport.authorityState !== "contained" ||
            pmReport.role !== "PM" ||
            pmReport.inputRevision !== core.inputRevision
          )
            throw new BlockedError(
              "restart cannot prove the persisted PM report accepted the current plan",
            );
        } else {
          let recoveryId: string | undefined;
          if (pmWork?.state === "pending")
            core.markReady(context(core, credential), pmWorkItemId);
          else if (pmWork?.state === "blocked" && priorPmAssignment) {
            if (priorPmAssignment.authorityState !== "contained")
              throw new BlockedError(
                "restart PM authority is not proven contained; refusing replacement",
              );
            recoveryId = core.pendingReplacementRecovery(
              pmWorkItemId,
              priorPmAssignment.assignmentId,
            );
            if (!recoveryId) {
              const recovery = core.recordRecovery(context(core, credential), {
                workItemId: pmWorkItemId,
                assignmentId: priorPmAssignment.assignmentId,
                recoveryId: randomUUID(),
                recoveryType: "worker_replacement",
                reason: "restart after proven PM runtime containment",
              });
              if (recovery.outcome !== "pending")
                throw new BlockedError(
                  `PM replacement was not authorized: ${recovery.outcome}`,
                );
              recoveryId = recovery.recoveryId;
            }
            core.markReady(context(core, credential), pmWorkItemId);
          } else if (pmWork?.state === "ready" && priorPmAssignment) {
            if (priorPmAssignment.authorityState !== "contained")
              throw new BlockedError(
                "restart PM authority is not proven contained; refusing replacement",
              );
            recoveryId = core.pendingReplacementRecovery(
              pmWorkItemId,
              priorPmAssignment.assignmentId,
            );
            if (!recoveryId)
              throw new BlockedError(
                "ready PM replacement has no pending contained recovery",
              );
          } else if (pmWork?.state !== "ready")
            throw new BlockedError(
              `PM work is ${pmWork?.state ?? "missing"} and cannot be safely assigned`,
            );
          if (dispatches >= plan.limits.maxDispatches)
            throw new BlockedError(
              "maxDispatches exhausted before PM replacement",
            );
          const pmAssignment = core.assignWorkItem(
            context(core, credential),
            pmWorkItemId,
            identities.PM.seatId,
            undefined,
            recoveryId,
          );
          pmAssignmentId = pmAssignment.assignmentId;
          const pmRuntime = await provisionRuntime(
            "PM",
            identities.PM.seatId,
            pmWorkItemId,
            pmAssignment.generation,
            pmAssignment.assignmentId,
          );
          runtimeCommands[pmRuntime.session.sessionId] = pmAssignment.commandId;
          if (stopping) blocker = "run canceled by signal";
          else if (Date.now() >= deadlineMs)
            blocker = "maxRunMs exceeded before PM dispatch";
          else {
            if (dispatches >= plan.limits.maxDispatches)
              throw new Error("maxDispatches exhausted before PM dispatch");
            dispatches += 1;
            await waitForDispatchPermission();
            core.transitionRuntimeSession(
              context(core, credential),
              pmRuntime.session.sessionId,
              "working",
            );
            await pmRuntime.adapter.dispatchAndStart(
              context(core, credential),
              pmAssignment.commandId,
            );
          }
          pmReport =
            stopping || blocker
              ? undefined
              : await waitForReport(pmWorkItemId, pmAssignment.assignmentId);
          if (!pmReport)
            blocker ??= stopping
              ? "run canceled by signal"
              : "PM report did not complete before the bounded run deadline";
        }
        if (!pmReport)
          blocker ??=
            "PM report did not complete before the bounded run deadline";
        else if (
          pmReport.assignmentId !== pmAssignmentId ||
          pmReport.role !== "PM" ||
          pmReport.inputRevision !== core.inputRevision
        )
          throw new Error(
            "PM report does not match the active plan-review assignment",
          );
        else {
          const pmSession = sessions[identities.PM.seatId];
          if (pmSession && !containedSessions.has(pmSession.sessionId))
            await containRuntime(pmSession, pmAssignmentId);
          const pmReply = objectRecord(
            parseJsonWithoutDuplicateMembers(pmReport.reply),
          );
          const acceptedPlanHash = pmReply?.planHash;
          if (acceptedPlanHash !== validateWorkflowPlan(plan).hash)
            blocker = "PM report did not affirm the exact validated plan hash";
          else if (pmWork?.state !== "accepted")
            core.acceptNonCandidateReport(
              context(core, credential),
              pmWorkItemId,
              pmAssignmentId,
            );
        }
        const planWasAccepted = core
          .statusSnapshot()
          .work.some(
            (item) =>
              item.workItemId === pmWorkItemId && item.state === "accepted",
          );
        if (!planWasAccepted)
          blocker ??=
            "Supervisor withheld because the PM plan was not accepted";
        const supervisorVerifiedFindings = new Map<
          string,
          { condition: string; evidence: string; assignmentId: string }
        >();
        const evaluateSupervisor = async (): Promise<string | undefined> => {
          supervisorVerifiedFindings.clear();
          let runtime: Awaited<ReturnType<typeof provisionRuntime>> | undefined;
          let assignmentId: string | undefined;
          let containmentProven = false;
          let supervisorWorkItemId: string | undefined;
          try {
            if (dispatches >= plan.limits.maxDispatches)
              throw new Error(
                "dispatch limit reached before Supervisor evaluation",
              );
            const evaluation = core.beginSupervisorEvaluation(
              context(core, credential),
            );
            const { window, snapshot } = core.captureSupervisorEvaluation(
              evaluation.eventUpperSequence,
              evaluation.targetEpoch,
            );
            const supervisorOverlap = window.hardViolations.find(
              (violation) => violation.seatId === identities.Supervisor.seatId,
            );
            if (supervisorOverlap) {
              const assignments = supervisorOverlap.assignmentIds
                .map((assignmentId) =>
                  core.latestAssignmentForRole("Supervisor", assignmentId),
                )
                .filter(
                  (
                    entry,
                  ): entry is NonNullable<
                    ReturnType<ControllerCore["latestAssignmentForRole"]>
                  > => entry !== undefined,
                );
              const source =
                assignments.find(
                  (entry) => entry.authorityState === "active",
                ) ?? assignments[0];
              const target = assignments.find(
                (entry) => entry.assignmentId !== source?.assignmentId,
              );
              if (!source || !target)
                throw new Error(
                  "Supervisor authority overlap has no durable assignment pair",
                );
              const requestedFindingId = randomUUID();
              const finding = core.createFinding(
                context(core, identities.Supervisor.credential),
                {
                  findingId: requestedFindingId,
                  workItemId: source.workItemId,
                  assignmentId: source.assignmentId,
                  generation: source.generation,
                  affectedWorkItemId: target.workItemId,
                  affectedSeatId: target.seatId,
                  affectedAssignmentId: target.assignmentId,
                  affectedGeneration: target.generation,
                  fingerprint: createHash("sha256")
                    .update(
                      JSON.stringify({
                        code: "overlapping_authority",
                        seatId: supervisorOverlap.seatId,
                        assignmentIds: [
                          ...supervisorOverlap.assignmentIds,
                        ].sort(),
                      }),
                    )
                    .digest("hex"),
                  severity: "critical",
                  evidence: { violation: supervisorOverlap },
                  requestedCorrection:
                    "Operator must restore exclusive Supervisor authority",
                  acknowledgementDeadline: new Date(
                    Math.min(deadlineMs, Date.now() + 15 * 60_000),
                  ).toISOString(),
                  resolutionCondition:
                    "Operator verifies exclusive Supervisor authority",
                  escalationRoute: "operator",
                },
              );
              const findingId = finding.findingId;
              escalateSupervisorOverlapFinding(
                core,
                findingId,
                source.authorityState,
                identities.Supervisor.credential,
                credential,
              );
              core.markSupervisionDegraded(
                context(core, credential),
                `Supervisor seat overlap recorded as finding ${findingId}`,
              );
              return `Supervisor seat authority overlap escalated to operator as finding ${findingId}`;
            }
            const visibleCandidateIds = new Set([
              ...acceptedCandidateByWorkItem.values(),
              ...candidateUnderReviewByWorkItem.values(),
            ]);
            const evidenceRoot = path.join(
              config.stateDirectory,
              "runtime",
              "evidence",
            );
            const supervisorEvidencePath = (artifactRef: string): string => {
              const relative = path.relative(evidenceRoot, artifactRef);
              if (
                !relative ||
                relative === ".." ||
                relative.startsWith(`..${path.sep}`) ||
                path.isAbsolute(relative)
              )
                throw new Error(
                  "Supervisor evidence artifact is outside the read-only evidence root",
                );
              return `/evidence/${relative.split(path.sep).join("/")}`;
            };
            const boundedContext = {
              taskId: plan.taskId,
              planHash: validateWorkflowPlan(plan).hash,
              plan: plan.slices.slice(0, 24),
              acceptanceCriteria: plan.acceptanceCriteria.slice(0, 32),
              epoch: evaluation.targetEpoch,
              eventUpperSequence: evaluation.eventUpperSequence,
              events: window.events.slice(),
              eventRefs: window.eventRefs,
              assignments: window.assignments,
              dependencies: window.dependencies,
              fingerprints: window.fingerprints,
              limits: {
                maxRunMs: plan.limits.maxRunMs,
                maxDispatches: plan.limits.maxDispatches,
                dispatchesUsed: dispatches,
                deadlineAt: new Date(deadlineMs).toISOString(),
              },
              supervision: snapshot.supervision,
              work: snapshot.work.slice(-24),
              roles: snapshot.roles.slice(-12),
              findings: snapshot.findings.slice(-24),
              candidateEvidence: snapshot.evidence
                .filter((entry) => visibleCandidateIds.has(entry.candidateId))
                .slice(-8)
                .map((entry) => ({
                  candidateId: entry.candidateId,
                  commitSha: entry.commitSha,
                  reportHash: entry.reportHash,
                  developerEvidence:
                    entry.developerEvidence
                      ?.slice(0, 8)
                      .map((item) => item.slice(0, 128)) ?? null,
                  verifierEvidence: entry.verifierEvidence
                    .slice(0, 16)
                    .map((item) => ({
                      evidenceId: item.evidenceId,
                      criterion: item.criterion.slice(0, 128),
                      passed: item.passed,
                      observation: item.observation?.slice(0, 128) ?? null,
                      exitStatus: item.exitStatus,
                      artifactPath: supervisorEvidencePath(item.artifactRef),
                      evidenceHash: item.evidenceHash,
                    })),
                })),
              finalVerification: snapshot.finalVerification
                .slice(-1)
                .map((entry) => ({
                  workItemId: entry.workItemId,
                  assignmentId: entry.assignmentId,
                  commitSha: entry.commitSha,
                  evidence: entry.evidence.slice(0, 16).map((item) => ({
                    criterion: item.criterion.slice(0, 128),
                    passed: item.passed,
                    observation: item.observation?.slice(0, 128) ?? null,
                    exitStatus: item.exitStatus,
                    artifactPath: supervisorEvidencePath(item.artifactRef),
                    evidenceHash: item.evidenceHash,
                  })),
                })),
            };
            let boundedJson = JSON.stringify(boundedContext);
            while (
              Buffer.byteLength(boundedJson) > 24_000 &&
              boundedContext.events.length > 8
            ) {
              boundedContext.events.shift();
              boundedJson = JSON.stringify(boundedContext);
            }
            if (Buffer.byteLength(boundedJson) > 24_000)
              throw new Error("bounded Supervisor context exceeds 24 KB");
            supervisorWorkItemId = `supervisor-${randomUUID()}`;
            core.createWorkItem(context(core, credential), {
              workItemId: supervisorWorkItemId,
              title: `Evaluate workflow epoch ${evaluation.targetEpoch}`,
              description: `Inspect this bounded workflow context, the read-only checkout at /workspace, and relevant artifacts under /evidence. Return JSON {"outcome":"pass"|"blocked","observation":"...","defectCode":"stable concise defect code for a blocked outcome","responsibleRole":"PM"|"Developer"|"Verifier","affectedAssignmentId":"exact assignment from context.eventRefs","affectedGeneration":number from context.eventRefs,"evidenceEventIds":["exact latest related event ID from context.eventRefs"],"verifiedFindings":[{"findingId":"...","condition":"exact recorded resolution condition","evidence":"new evidence supporting that condition"}]}. Every blocked report must include a stable defectCode (ASCII letters, digits, period, underscore or hyphen; 1-64 characters), name the exact affectedAssignmentId and affectedGeneration from context.eventRefs, and cite exactly that assignment's latest eventRef.eventId. Include verifiedFindings only when fresh evidence meets an open finding's exact resolution condition.`,
              requiredRole: "Supervisor",
            });
            core.markReady(context(core, credential), supervisorWorkItemId);
            const assignment = core.assignWorkItem(
              context(core, credential),
              supervisorWorkItemId,
              identities.Supervisor.seatId,
            );
            assignmentId = assignment.assignmentId;
            core.bindSupervisorEvaluation(context(core, credential), {
              assignmentId: assignment.assignmentId,
              generation: assignment.generation,
              targetEpoch: evaluation.targetEpoch,
              eventUpperSequence: evaluation.eventUpperSequence,
            });
            const evidenceCandidates = snapshot.evidence
              .filter((entry) => visibleCandidateIds.has(entry.candidateId))
              .slice(-12)
              .flatMap((entry) => {
                const workspace = candidateWorkspaces[entry.candidateId];
                return workspace
                  ? [{ workspace, commitSha: entry.commitSha }]
                  : [];
              });
            const supervisorRuntime = await provisionRuntime(
              "Supervisor",
              identities.Supervisor.seatId,
              supervisorWorkItemId,
              assignment.generation,
              assignment.assignmentId,
              finalVerificationSource ? [] : evidenceCandidates,
              finalVerificationSource,
            );
            runtime = supervisorRuntime;
            runtimeCommands[supervisorRuntime.session.sessionId] =
              assignment.commandId;
            if (stopping || Date.now() >= deadlineMs)
              throw new Error(
                "Supervisor evaluation exceeded the run deadline",
              );
            await waitForDispatchPermission();
            dispatches += 1;
            core.transitionRuntimeSession(
              context(core, credential),
              supervisorRuntime.session.sessionId,
              "working",
            );
            await supervisorRuntime.adapter.dispatchAndStart(
              context(core, credential),
              assignment.commandId,
            );
            const report = await waitForReport(
              supervisorWorkItemId,
              assignment.assignmentId,
            );
            if (
              !report ||
              report.assignmentId !== assignment.assignmentId ||
              report.role !== "Supervisor" ||
              report.inputRevision !== core.inputRevision
            )
              throw new Error(
                "Supervisor evaluation report is missing or stale",
              );
            const reply = objectRecord(
              parseJsonWithoutDuplicateMembers(report.reply),
            );
            if (
              typeof reply?.observation !== "string" ||
              !reply.observation.trim() ||
              (reply.outcome !== "pass" && reply.outcome !== "blocked")
            )
              throw new Error(
                "Supervisor returned an invalid structured evaluation",
              );
            if (
              reply.verifiedFindings !== undefined &&
              !Array.isArray(reply.verifiedFindings)
            )
              throw new Error("Supervisor verifiedFindings must be an array");
            for (const entry of Array.isArray(reply.verifiedFindings)
              ? reply.verifiedFindings
              : []) {
              const verified = objectRecord(entry);
              const finding = snapshot.findings.find(
                (item) => item.findingId === verified?.findingId,
              );
              if (
                !verified ||
                !finding ||
                finding.state !== "correcting" ||
                verified?.condition !== finding.resolutionCondition ||
                typeof verified.evidence !== "string" ||
                !verified.evidence.trim() ||
                verified.evidence.length > 2048 ||
                supervisorVerifiedFindings.has(finding.findingId)
              )
                throw new Error(
                  "Supervisor verified finding must name an open finding, exact condition, and bounded evidence",
                );
              supervisorVerifiedFindings.set(finding.findingId, {
                condition: finding.resolutionCondition,
                evidence: verified.evidence,
                assignmentId: assignment.assignmentId,
              });
            }
            const pendingCorrection =
              window.hardViolations.length === 0
                ? snapshot.findings.find(
                    (finding) =>
                      finding.state === "reported" &&
                      reply.outcome !== "blocked" &&
                      (snapshot.work.some(
                        (work) =>
                          work.workItemId ===
                            `correction-${finding.findingId}` &&
                          ["pending", "ready", "blocked"].includes(work.state),
                      ) ||
                        !snapshot.work.some(
                          (work) =>
                            work.workItemId ===
                            `correction-${finding.findingId}`,
                        )),
                  )
                : undefined;
            const blocked =
              reply.outcome === "blocked" ||
              window.hardViolations.length > 0 ||
              pendingCorrection !== undefined;
            const observation =
              window.hardViolations.length > 0
                ? JSON.stringify({
                    deterministicViolations: window.hardViolations,
                    diagnosis: reply.observation,
                  })
                : reply.observation;
            let affected:
              | NonNullable<
                  ReturnType<ControllerCore["latestAssignmentForRole"]>
                >
              | undefined;
            let findingId: string | undefined;
            let correctionWorkItemId: string | undefined;
            let isDeduplicated = false;
            let hardViolationTarget:
              | NonNullable<
                  ReturnType<ControllerCore["latestAssignmentForRole"]>
                >
              | undefined;
            if (window.hardViolations.length > 0) {
              const boundedHardViolationTarget = window.assignments.find(
                (entry) =>
                  ["PM", "Developer", "Verifier"].includes(entry.role) &&
                  window.hardViolations.some((violation) =>
                    violation.assignmentIds.includes(entry.assignmentId),
                  ),
              );
              hardViolationTarget = (() => {
                if (boundedHardViolationTarget) {
                  for (const role of ["PM", "Developer", "Verifier"] as const) {
                    const exactAssignment = core.latestAssignmentForRole(
                      role,
                      boundedHardViolationTarget.assignmentId,
                    );
                    if (exactAssignment) return exactAssignment;
                  }
                }
                for (const violation of window.hardViolations) {
                  for (const assignmentId of violation.assignmentIds) {
                    for (const role of [
                      "PM",
                      "Developer",
                      "Verifier",
                    ] as const) {
                      const exactAssignment = core.latestAssignmentForRole(
                        role,
                        assignmentId,
                      );
                      if (exactAssignment) return exactAssignment;
                    }
                  }
                }
                return undefined;
              })();
              if (!hardViolationTarget) {
                const supervisorOverlap = window.hardViolations.find(
                  (violation) =>
                    violation.seatId === identities.Supervisor.seatId,
                );
                hardViolationTarget = supervisorOverlap?.assignmentIds
                  .map((id) => core.latestAssignmentForRole("Supervisor", id))
                  .find((entry) => entry !== undefined);
              }
              if (!hardViolationTarget)
                throw new Error(
                  "hard-state violation has no exact worker assignment to record",
                );
            }
            if (blocked) {
              const pendingAffected = pendingCorrection
                ? (["PM", "Developer", "Verifier"] as const)
                    .map((role) =>
                      core.latestAssignmentForRole(
                        role,
                        pendingCorrection.affectedAssignmentId,
                      ),
                    )
                    .find((entry) => entry !== undefined)
                : undefined;
              const responsibleRole = hardViolationTarget
                ? hardViolationTarget.role
                : pendingAffected
                  ? (pendingAffected.role as "PM" | "Developer" | "Verifier")
                  : ["PM", "Developer", "Verifier"].includes(
                        String(reply.responsibleRole),
                      )
                    ? (reply.responsibleRole as "PM" | "Developer" | "Verifier")
                    : undefined;
              if (!responsibleRole)
                throw new Error(
                  "Supervisor blocked evaluation without a responsible worker role",
                );
              const requestedAssignmentId =
                pendingCorrection?.affectedAssignmentId ??
                hardViolationTarget?.assignmentId ??
                reply.affectedAssignmentId;
              if (
                typeof requestedAssignmentId !== "string" ||
                !requestedAssignmentId.trim()
              )
                throw new Error(
                  "Supervisor finding must identify its exact affected assignment",
                );
              const affected =
                pendingAffected ??
                core.latestAssignmentForRole(
                  responsibleRole,
                  requestedAssignmentId,
                ) ??
                undefined;
              if (!affected)
                throw new Error(
                  "Supervisor finding has no exact affected worker assignment",
                );
              const affectedGeneration =
                hardViolationTarget?.generation ??
                pendingAffected?.generation ??
                window.eventRefs.find(
                  (entry) => entry.assignmentId === affected.assignmentId,
                )?.generation;
              if (
                affectedGeneration === undefined ||
                affected.generation !== affectedGeneration ||
                (hardViolationTarget === undefined &&
                  pendingAffected === undefined &&
                  reply.affectedGeneration !== affectedGeneration)
              )
                throw new Error(
                  "Supervisor finding must bind the affected assignment generation from its evaluation context",
                );
              if (pendingCorrection) {
                findingId = pendingCorrection.findingId;
                correctionWorkItemId = `correction-${findingId}`;
                isDeduplicated = true;
              } else {
                let evidenceEventIds: string[] = [];
                if (window.hardViolations.length === 0) {
                  const latestRelatedEvent = window.eventRefs.find(
                    (entry) => entry.assignmentId === affected.assignmentId,
                  );
                  if (
                    !Array.isArray(reply.evidenceEventIds) ||
                    reply.evidenceEventIds.length !== 1 ||
                    typeof reply.evidenceEventIds[0] !== "string" ||
                    reply.evidenceEventIds[0] !== latestRelatedEvent?.eventId
                  )
                    throw new Error(
                      "Supervisor finding must cite the latest material event for its affected assignment or work item",
                    );
                  evidenceEventIds = [latestRelatedEvent.eventId];
                }
                const fingerprintIdentity =
                  window.hardViolations.length > 0
                    ? JSON.stringify(
                        window.hardViolations
                          .map((violation) => ({
                            code: violation.code,
                            seatId: violation.seatId,
                            assignmentIds: [...violation.assignmentIds].sort(),
                          }))
                          .sort((left, right) =>
                            JSON.stringify(left).localeCompare(
                              JSON.stringify(right),
                            ),
                          ),
                      )
                    : findingDefectIdentity(
                        evidenceEventIds[0],
                        reply.defectCode,
                      );
                const requestedFindingId = randomUUID();
                const finding = core.createFinding(
                  context(core, identities.Supervisor.credential),
                  {
                    findingId: requestedFindingId,
                    workItemId: supervisorWorkItemId,
                    assignmentId: assignment.assignmentId,
                    generation: assignment.generation,
                    affectedWorkItemId: affected.workItemId,
                    affectedSeatId: affected.seatId,
                    affectedAssignmentId: affected.assignmentId,
                    affectedGeneration,
                    fingerprint: createFindingFingerprint(
                      affected.assignmentId,
                      fingerprintIdentity,
                    ),
                    severity: "high",
                    evidence: { observation, evidenceEventIds },
                    requestedCorrection: observation.slice(0, 2048),
                    acknowledgementDeadline: new Date(
                      Math.min(deadlineMs, Date.now() + 15 * 60_000),
                    ).toISOString(),
                    resolutionCondition:
                      "The bound correction is accepted and a fresh independent Supervisor checkpoint confirms this observation is resolved",
                    escalationRoute: "operator",
                  },
                );
                findingId = finding.findingId;
                correctionWorkItemId = `correction-${findingId}`;
                isDeduplicated = findingId !== requestedFindingId;
                if (!isDeduplicated)
                  core.transitionFinding(
                    context(core, identities.Supervisor.credential),
                    findingId,
                    "reported",
                    { observation },
                  );
              }
              if (affected.role === "Supervisor") {
                const state = core
                  .statusSnapshot()
                  .findings.find(
                    (entry) => entry.findingId === findingId,
                  )?.state;
                if (state === "reported")
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "escalated",
                    {
                      reason:
                        "overlapping Supervisor authority requires operator intervention",
                    },
                  );
                await containRuntime(
                  supervisorRuntime.session,
                  assignment.assignmentId,
                );
                containmentProven = true;
                core.markSupervisionDegraded(
                  context(core, credential),
                  `Supervisor seat overlap recorded as finding ${findingId}`,
                );
                return `Supervisor seat authority overlap escalated to operator as finding ${findingId}`;
              }
              const currentFindingState = core
                .statusSnapshot()
                .findings.find((entry) => entry.findingId === findingId)?.state;
              if (
                currentFindingState !== "resolved" &&
                affected.authorityState !== "contained"
              ) {
                const targetSession = allSessions.find(
                  (session) =>
                    runtimeAssignments[session.sessionId] ===
                    affected!.assignmentId,
                );
                if (!targetSession)
                  throw new Error(
                    `Supervisor finding ${findingId} is durable but affected authority is ${affected.authorityState} without an identifiable runtime; correction is paused`,
                  );
                await containRuntime(targetSession, affected.assignmentId);
                throw new Error(
                  `Supervisor finding ${findingId} is durable; the affected runtime was contained and needs a fresh Supervisor evaluation before correction binding`,
                );
              }
              const existingCorrectionWork = core
                .statusSnapshot()
                .work.some((work) => work.workItemId === correctionWorkItemId);
              if (
                !existingCorrectionWork &&
                core
                  .statusSnapshot()
                  .findings.some(
                    (entry) =>
                      entry.findingId === findingId &&
                      entry.state === "reported",
                  )
              ) {
                core.createWorkItem(context(core, credential), {
                  workItemId: correctionWorkItemId,
                  findingId,
                  title: `Correct Supervisor finding ${findingId}`,
                  description: `Acknowledge this finding and provide an assignment-bound correction or dispute: ${observation.slice(0, 2048)}${affected.role === "PM" ? " A PM response cannot revise the accepted active run plan; a proposed plan change will be escalated to the operator rather than treated as applied." : ""}`,
                  requiredRole: affected.role,
                  ...(affected.role === "Developer"
                    ? {
                        acceptanceCriteria:
                          plan.slices.find(
                            (slice) =>
                              workflowWorkItemId(plan.taskId, slice.id) ===
                              affected!.workItemId,
                          )?.acceptanceCriteria ?? plan.acceptanceCriteria,
                      }
                    : affected.role === "Verifier"
                      ? affected.parentWorkItemId
                        ? {
                            parentWorkItemId: affected.parentWorkItemId,
                          }
                        : {
                            finalVerification: true,
                            acceptanceCriteria: plan.acceptanceCriteria,
                          }
                      : {}),
                });
              }
            }
            await containRuntime(
              supervisorRuntime.session,
              assignment.assignmentId,
            );
            containmentProven = true;
            core.acceptNonCandidateReport(
              context(core, credential),
              supervisorWorkItemId,
              assignment.assignmentId,
            );
            core.recordSupervisorCheckpoint(context(core, credential), {
              assignmentId: assignment.assignmentId,
              generation: assignment.generation,
              targetEpoch: evaluation.targetEpoch,
              eventUpperSequence: evaluation.eventUpperSequence,
              fingerprint: createHash("sha256")
                .update(boundedJson)
                .digest("hex"),
            });
            if (blocked) {
              supervisorMustPause = true;
              if (isDeduplicated) {
                if (!findingId)
                  throw new Error(
                    "deduplicated Supervisor finding ID is missing",
                  );
                const existing = core
                  .statusSnapshot()
                  .findings.find((entry) => entry.findingId === findingId);
                if (existing?.state === "resolved") {
                  const unresolved = core
                    .statusSnapshot()
                    .findings.some((entry) => entry.state !== "resolved");
                  supervisorMustPause = unresolved;
                  return unresolved
                    ? `Supervisor finding ${findingId} was suppressed during cooldown; other findings remain unresolved`
                    : undefined;
                }
                if (
                  existing?.state === "reported" &&
                  Date.now() >= Date.parse(existing.acknowledgementDeadline)
                ) {
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "escalated",
                    { reason: "acknowledgement deadline expired" },
                  );
                  return `Supervisor finding ${findingId} escalated after its acknowledgement deadline`;
                }
                const correctionPending = core
                  .statusSnapshot()
                  .work.some(
                    (work) =>
                      work.workItemId === correctionWorkItemId &&
                      ["pending", "ready", "blocked"].includes(work.state),
                  );
                if (
                  existing?.state !== "reported" ||
                  !correctionPending ||
                  !affected ||
                  !correctionWorkItemId
                )
                  return `Supervisor finding ${findingId} remains unresolved; duplicate observation was not dispatched again`;
              }
              if (!affected || !findingId || !correctionWorkItemId)
                throw new Error("Supervisor correction target is unavailable");
              if (dispatches >= plan.limits.maxDispatches) {
                core.transitionFinding(
                  context(core, credential),
                  findingId,
                  "escalated",
                  { reason: "correction dispatch limit reached" },
                );
                return `Supervisor finding escalated; correction dispatch limit reached: ${observation}`;
              }
              const activeFinding = core
                .statusSnapshot()
                .findings.find((entry) => entry.findingId === findingId);
              const findingDeadlineMs = activeFinding
                ? Date.parse(activeFinding.acknowledgementDeadline)
                : Number.NaN;
              if (!Number.isFinite(findingDeadlineMs))
                throw new Error("finding acknowledgement deadline is invalid");
              if (Date.now() >= findingDeadlineMs) {
                core.transitionFinding(
                  context(core, credential),
                  findingId,
                  "escalated",
                  {
                    reason: "acknowledgement deadline expired before dispatch",
                  },
                );
                return `Supervisor finding ${findingId} escalated before correction dispatch because its acknowledgement deadline expired`;
              }
              let correctionRuntime:
                Awaited<ReturnType<typeof provisionRuntime>> | undefined;
              let correctionAssignmentId: string | undefined;
              let correctionContained = false;
              try {
                const finalVerificationCorrection =
                  affected.role === "Verifier" &&
                  affected.parentWorkItemId === null;
                if (finalVerificationCorrection) {
                  const finalSliceWorkItemIds = plan.slices.map((slice) =>
                    workflowWorkItemId(plan.taskId, slice.id),
                  );
                  if (
                    finalSliceWorkItemIds.some(
                      (workItemId) =>
                        !acceptedCandidateByWorkItem.has(workItemId),
                    )
                  )
                    throw new Error(
                      "final Verifier correction requires every accepted slice",
                    );
                  for (const workItemId of finalSliceWorkItemIds)
                    core.addDependency(
                      context(core, credential),
                      correctionWorkItemId,
                      workItemId,
                    );
                }
                core.markReady(context(core, credential), correctionWorkItemId);
                const acceptedDeveloperCandidateId =
                  affected.role === "Developer"
                    ? acceptedCandidateByWorkItem.get(affected.workItemId)
                    : undefined;
                const boundCandidate =
                  affected.role === "Verifier" && !finalVerificationCorrection
                    ? core.candidateForAssignment(affected.assignmentId)
                    : undefined;
                const acceptedDeveloperCandidate = acceptedDeveloperCandidateId
                  ? core
                      .statusSnapshot()
                      .evidence.find(
                        (entry) =>
                          entry.candidateId === acceptedDeveloperCandidateId,
                      )
                  : undefined;
                if (
                  acceptedDeveloperCandidateId &&
                  (!acceptedDeveloperCandidate ||
                    !candidateWorkspaces[acceptedDeveloperCandidateId])
                )
                  throw new Error(
                    "Developer correction has no exact accepted candidate checkout",
                  );
                if (
                  affected.role === "Verifier" &&
                  !finalVerificationCorrection &&
                  !boundCandidate
                )
                  throw new Error(
                    "Verifier correction has no candidate bound to its affected assignment",
                  );
                if (finalVerificationCorrection && !finalVerificationSource)
                  throw new Error(
                    "final Verifier correction has no exact composed checkout",
                  );
                const correctionAssignment = core.assignWorkItem(
                  context(core, credential),
                  correctionWorkItemId,
                  affected.seatId,
                  boundCandidate?.candidateId,
                );
                correctionAssignmentId = correctionAssignment.assignmentId;
                correctionRuntime = await (() => {
                  const candidate =
                    boundCandidate ?? acceptedDeveloperCandidate;
                  const candidateWorkspace = candidate
                    ? candidateWorkspaces[candidate.candidateId]
                    : undefined;
                  const exactCandidate =
                    candidate && candidateWorkspace
                      ? {
                          workspace: candidateWorkspace,
                          commitSha: candidate.commitSha,
                        }
                      : finalVerificationCorrection
                        ? finalVerificationSource
                        : undefined;
                  if (affected.role === "Verifier" && !exactCandidate)
                    throw new Error(
                      "Verifier correction has no exact bound workspace",
                    );
                  return provisionRuntime(
                    affected.role,
                    affected.seatId,
                    correctionWorkItemId,
                    correctionAssignment.generation,
                    correctionAssignment.assignmentId,
                    [],
                    exactCandidate,
                  );
                })();
                runtimeCommands[correctionRuntime.session.sessionId] =
                  correctionAssignment.commandId;
                if (stopping || Date.now() >= deadlineMs)
                  throw new Error(
                    "correction dispatch exceeded the run boundary",
                  );
                await waitForDispatchPermission();
                dispatches += 1;
                core.transitionRuntimeSession(
                  context(core, credential),
                  correctionRuntime.session.sessionId,
                  "working",
                );
                await correctionRuntime.adapter.dispatchAndStart(
                  context(core, credential),
                  correctionAssignment.commandId,
                );
                const correctionReport = await waitForReport(
                  correctionWorkItemId,
                  correctionAssignment.assignmentId,
                  Math.min(deadlineMs, findingDeadlineMs),
                );
                if (
                  !correctionReport ||
                  correctionReport.assignmentId !==
                    correctionAssignment.assignmentId ||
                  correctionReport.role !== affected.role ||
                  correctionReport.inputRevision !== core.inputRevision
                )
                  throw new Error(
                    "correction response is missing, stale, or mismatched",
                  );
                const correctionReply = objectRecord(
                  parseJsonWithoutDuplicateMembers(correctionReport.reply),
                );
                if (
                  typeof correctionReply?.acknowledgment !== "string" ||
                  !correctionReply.acknowledgment.trim() ||
                  typeof correctionReply.response !== "string" ||
                  !correctionReply.response.trim() ||
                  (correctionReply.disposition !== "correcting" &&
                    correctionReply.disposition !== "disputed")
                )
                  throw new Error(
                    "correction response must acknowledge and provide a correction or dispute",
                  );
                const correctionContext = context(
                  core,
                  identities[affected.role].credential,
                );
                core.transitionFinding(
                  correctionContext,
                  findingId,
                  "acknowledged",
                  {
                    acknowledgment: correctionReply.acknowledgment,
                    response: correctionReply.response,
                  },
                );
                if (correctionReply.disposition === "disputed") {
                  core.transitionFinding(
                    correctionContext,
                    findingId,
                    "disputed",
                    { response: correctionReply.response },
                  );
                } else {
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "correcting",
                    { workItemId: correctionWorkItemId },
                  );
                }
                await containRuntime(
                  correctionRuntime.session,
                  correctionAssignment.assignmentId,
                );
                correctionContained = true;
                if (correctionReply.disposition === "disputed") {
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "escalated",
                    { reason: `worker dispute: ${correctionReply.response}` },
                  );
                  return `Supervisor finding ${findingId} was disputed and escalated to the operator`;
                }
                if (
                  correctionReply.disposition === "correcting" &&
                  affected.role === "PM"
                ) {
                  core.acceptNonCandidateReport(
                    context(core, credential),
                    correctionWorkItemId,
                    correctionAssignment.assignmentId,
                  );
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "escalated",
                    {
                      reason:
                        "PM correction response was recorded, but the accepted active run plan cannot be revised by a correction assignment",
                    },
                  );
                  return `PM correction response for finding ${findingId} was recorded; the unchanged accepted plan was escalated to the operator`;
                }
                if (
                  correctionReply.disposition === "correcting" &&
                  affected.role === "Verifier"
                ) {
                  if (finalVerificationCorrection) {
                    const finalSource = finalVerificationSource!;
                    const criteria = plan.acceptanceCriteria;
                    const verifierReply = objectRecord(
                      parseJsonWithoutDuplicateMembers(correctionReport.reply),
                    );
                    if (
                      verifierReply?.commitSha !== finalSource.commitSha ||
                      verifierReply.startingSha !== finalSource.commitSha ||
                      !Array.isArray(verifierReply.evidence) ||
                      verifierReply.evidence.length !== criteria.length
                    )
                      throw new Error(
                        "final Verifier correction report does not bind the exact composed checkout and parent criteria",
                      );
                    const checkedHead = spawnSync(
                      "git",
                      [
                        "--no-replace-objects",
                        "-c",
                        "core.fsmonitor=false",
                        "-c",
                        "core.hooksPath=/dev/null",
                        `--git-dir=${path.join(correctionRuntime.session.workspace, ".git")}`,
                        `--work-tree=${correctionRuntime.session.workspace}`,
                        "-C",
                        correctionRuntime.session.workspace,
                        "rev-parse",
                        "HEAD",
                      ],
                      {
                        encoding: "utf8",
                        timeout: 10_000,
                        env: verifiedGitEnv,
                      },
                    );
                    if (
                      checkedHead.status !== 0 ||
                      checkedHead.stdout.trim().toLowerCase() !==
                        finalSource.commitSha.toLowerCase() ||
                      correctionRuntime.baseSha.toLowerCase() !==
                        finalSource.commitSha.toLowerCase()
                    )
                      throw new Error(
                        "final Verifier correction did not inspect the exact composed commit",
                      );
                    assertTrackedCheckoutMatchesHead(
                      correctionRuntime.session.workspace,
                      finalSource.commitSha,
                    );
                    const evidence: EvidenceInput[] = [];
                    const seen = new Set<string>();
                    const evidenceDirectory =
                      correctionRuntime.evidenceDirectory;
                    if (!evidenceDirectory)
                      throw new Error(
                        "final Verifier correction evidence directory is unavailable",
                      );
                    for (const raw of verifierReply.evidence) {
                      const entry = objectRecord(raw);
                      if (
                        typeof entry?.criterion !== "string" ||
                        !criteria.includes(entry.criterion) ||
                        seen.has(entry.criterion) ||
                        entry.passed !== true ||
                        typeof entry.observation !== "string" ||
                        !entry.observation.trim() ||
                        !Number.isInteger(entry.exitStatus) ||
                        entry.exitStatus !== 0 ||
                        typeof entry.artifactRef !== "string" ||
                        !entry.artifactRef.startsWith("/evidence/")
                      )
                        throw new Error(
                          "final Verifier correction evidence must pass every exact parent criterion",
                        );
                      seen.add(entry.criterion);
                      evidence.push({
                        evidenceId: randomUUID(),
                        candidateId: finalSource.commitSha,
                        criterion: entry.criterion,
                        passed: true,
                        observation: entry.observation,
                        exitStatus: 0,
                        artifactRef: verifiedEvidencePath(
                          evidenceDirectory,
                          entry.artifactRef,
                        ),
                      });
                    }
                    if (seen.size !== criteria.length)
                      throw new Error(
                        "final Verifier correction omitted a parent criterion",
                      );
                    const correctionReviewBlocker = await evaluateSupervisor();
                    if (correctionReviewBlocker)
                      return `Supervisor correction report remains unaccepted: ${correctionReviewBlocker}`;
                    core.acceptFinalVerification(context(core, credential), {
                      workItemId: correctionWorkItemId,
                      assignmentId: correctionAssignment.assignmentId,
                      commitSha: finalSource.commitSha,
                      evidence,
                    });
                    const verificationBlocker = await evaluateSupervisor();
                    if (verificationBlocker)
                      return `Supervisor correction requires further review: ${verificationBlocker}`;
                    const verifiedCondition =
                      supervisorVerifiedFindings.get(findingId);
                    const finding = core
                      .statusSnapshot()
                      .findings.find((entry) => entry.findingId === findingId);
                    const correctionEvidence =
                      core.latestAcceptedWorkEvent(correctionWorkItemId);
                    if (!verifiedCondition || !finding || !correctionEvidence)
                      return `Supervisor did not verify the recorded condition for finding ${findingId}`;
                    core.transitionFinding(
                      context(core, credential),
                      findingId,
                      "resolved",
                      {
                        condition: finding.resolutionCondition,
                        evidenceEventId: correctionEvidence.eventId,
                        assignmentId: correctionAssignment.assignmentId,
                        generation: correctionAssignment.generation,
                        evidence: {
                          supervisorCheckpointAssignmentId:
                            verifiedCondition.assignmentId,
                          supervisorVerificationAssignmentId:
                            verifiedCondition.assignmentId,
                          correctionEventId: correctionEvidence.eventId,
                          condition: finding.resolutionCondition,
                          supervisorVerificationEvidence:
                            verifiedCondition.evidence,
                        },
                      },
                    );
                    const finalReviewBlocker = await evaluateSupervisor();
                    if (finalReviewBlocker)
                      return `Resolved finding requires a fresh checkpoint: ${finalReviewBlocker}`;
                    if (
                      !core
                        .statusSnapshot()
                        .findings.some((entry) => entry.state !== "resolved")
                    )
                      supervisorMustPause = false;
                    return undefined;
                  }
                  const candidate = boundCandidate!;
                  const candidateWorkspace =
                    candidateWorkspaces[candidate.candidateId];
                  if (!candidateWorkspace)
                    throw new Error(
                      "Verifier correction candidate workspace is unavailable",
                    );
                  const candidateSlice = plan.slices.find(
                    (slice) =>
                      workflowWorkItemId(plan.taskId, slice.id) ===
                      candidate.workItemId,
                  );
                  const criteria =
                    candidateSlice?.acceptanceCriteria ??
                    plan.acceptanceCriteria;
                  const verifierReply = objectRecord(
                    parseJsonWithoutDuplicateMembers(correctionReport.reply),
                  );
                  if (
                    verifierReply?.candidateId !== candidate.candidateId ||
                    verifierReply.commitSha !== candidate.commitSha ||
                    verifierReply.startingSha !== candidate.commitSha ||
                    !Array.isArray(verifierReply.evidence) ||
                    verifierReply.evidence.length !== criteria.length
                  )
                    throw new Error(
                      "Verifier correction report does not bind the exact candidate checkout and every criterion",
                    );
                  const seen = new Set<string>();
                  const evidence: EvidenceInput[] = [];
                  const evidenceDirectory = correctionRuntime.evidenceDirectory;
                  if (!evidenceDirectory)
                    throw new Error(
                      "Verifier correction evidence directory is unavailable",
                    );
                  for (const raw of verifierReply.evidence) {
                    const entry = objectRecord(raw);
                    if (
                      typeof entry?.criterion !== "string" ||
                      !criteria.includes(entry.criterion) ||
                      seen.has(entry.criterion) ||
                      entry.passed !== true ||
                      typeof entry.observation !== "string" ||
                      !entry.observation.trim() ||
                      !Number.isInteger(entry.exitStatus) ||
                      entry.exitStatus !== 0 ||
                      typeof entry.artifactRef !== "string" ||
                      !entry.artifactRef.startsWith("/evidence/")
                    )
                      throw new Error(
                        "Verifier correction evidence must pass each exact criterion with a bounded artifact",
                      );
                    seen.add(entry.criterion);
                    evidence.push({
                      evidenceId: randomUUID(),
                      candidateId: candidate.candidateId,
                      criterion: entry.criterion,
                      passed: true,
                      observation: entry.observation,
                      exitStatus: 0,
                      artifactRef: verifiedEvidencePath(
                        evidenceDirectory,
                        entry.artifactRef,
                      ),
                    });
                  }
                  if (seen.size !== criteria.length)
                    throw new Error(
                      "Verifier correction omitted an acceptance criterion",
                    );
                  const checkedHead = spawnSync(
                    "git",
                    [
                      "--no-replace-objects",
                      "-c",
                      "core.fsmonitor=false",
                      "-c",
                      "core.hooksPath=/dev/null",
                      `--git-dir=${path.join(correctionRuntime.session.workspace, ".git")}`,
                      `--work-tree=${correctionRuntime.session.workspace}`,
                      "-C",
                      correctionRuntime.session.workspace,
                      "rev-parse",
                      "HEAD",
                    ],
                    {
                      encoding: "utf8",
                      timeout: 10_000,
                      env: verifiedGitEnv,
                    },
                  );
                  if (
                    checkedHead.status !== 0 ||
                    checkedHead.stdout.trim().toLowerCase() !==
                      candidate.commitSha.toLowerCase() ||
                    correctionRuntime.baseSha.toLowerCase() !==
                      candidate.commitSha.toLowerCase()
                  )
                    throw new Error(
                      "Verifier correction did not inspect the exact immutable candidate",
                    );
                  assertTrackedCheckoutMatchesHead(
                    correctionRuntime.session.workspace,
                    candidate.commitSha,
                  );
                  core.recordEvidenceBatch(
                    correctionContext,
                    correctionAssignment.assignmentId,
                    evidence,
                  );
                  const correctionReviewBlocker = await evaluateSupervisor();
                  if (correctionReviewBlocker)
                    return `Supervisor correction report remains unaccepted: ${correctionReviewBlocker}`;
                  const acceptedParent =
                    core
                      .statusSnapshot()
                      .work.find(
                        (work) => work.workItemId === candidate.workItemId,
                      )?.state === "accepted";
                  if (acceptedParent) {
                    core.acceptVerifierCorrection(context(core, credential), {
                      findingId,
                      workItemId: correctionWorkItemId,
                      assignmentId: correctionAssignment.assignmentId,
                      candidateId: candidate.candidateId,
                    });
                  } else {
                    core.acceptCandidate(
                      context(core, credential),
                      candidate.workItemId,
                      candidate.candidateId,
                    );
                    acceptedCandidateByWorkItem.set(
                      candidate.workItemId,
                      candidate.candidateId,
                    );
                  }
                  const verificationBlocker = await evaluateSupervisor();
                  if (verificationBlocker)
                    return `Supervisor correction requires further review: ${verificationBlocker}`;
                  const verifiedCondition =
                    supervisorVerifiedFindings.get(findingId);
                  const finding = core
                    .statusSnapshot()
                    .findings.find((entry) => entry.findingId === findingId);
                  const correctionEvidence = core.latestAcceptedWorkEvent(
                    acceptedParent
                      ? correctionWorkItemId
                      : candidate.workItemId,
                  );
                  if (!verifiedCondition || !finding || !correctionEvidence)
                    return `Supervisor did not verify the recorded condition for finding ${findingId}`;
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "resolved",
                    {
                      condition: finding.resolutionCondition,
                      evidenceEventId: correctionEvidence.eventId,
                      assignmentId: correctionAssignment.assignmentId,
                      generation: correctionAssignment.generation,
                      evidence: {
                        supervisorCheckpointAssignmentId:
                          verifiedCondition.assignmentId,
                        supervisorVerificationAssignmentId:
                          verifiedCondition.assignmentId,
                        correctionEventId: correctionEvidence.eventId,
                        condition: finding.resolutionCondition,
                        supervisorVerificationEvidence:
                          verifiedCondition.evidence,
                      },
                    },
                  );
                  const finalReviewBlocker = await evaluateSupervisor();
                  if (finalReviewBlocker)
                    return `Resolved finding requires a fresh checkpoint: ${finalReviewBlocker}`;
                  if (
                    !core
                      .statusSnapshot()
                      .findings.some((entry) => entry.state !== "resolved")
                  )
                    supervisorMustPause = false;
                  return undefined;
                }
                if (
                  correctionReply.disposition === "correcting" &&
                  affected.role === "Developer"
                ) {
                  const developerReply = objectRecord(
                    parseJsonWithoutDuplicateMembers(correctionReport.reply),
                  );
                  const candidateId = developerReply?.candidateId;
                  const commitSha = developerReply?.commitSha;
                  const baseSha = developerReply?.baseSha;
                  const changedScope = developerReply?.changedScope;
                  const limitations = developerReply?.limitations;
                  const developerEvidence = developerReply?.evidence;
                  const slice = plan.slices.find(
                    (entry) =>
                      workflowWorkItemId(plan.taskId, entry.id) ===
                      affected.workItemId,
                  );
                  if (
                    !slice ||
                    candidateId !==
                      `candidate-${correctionAssignment.assignmentId}` ||
                    typeof commitSha !== "string" ||
                    !/^[a-f0-9]{40}([a-f0-9]{24})?$/i.test(commitSha) ||
                    baseSha !== correctionRuntime.baseSha ||
                    !Array.isArray(changedScope) ||
                    changedScope.some((entry) => typeof entry !== "string") ||
                    !Array.isArray(limitations) ||
                    limitations.some((entry) => typeof entry !== "string") ||
                    !Array.isArray(developerEvidence) ||
                    developerEvidence.length === 0 ||
                    developerEvidence.length > 64 ||
                    developerEvidence.some(
                      (entry) =>
                        typeof entry !== "string" ||
                        !entry.trim() ||
                        entry.length > 4096,
                    )
                  )
                    throw new Error(
                      "Developer correction report lacks a valid candidate assignment, base, scope, or evidence",
                    );
                  const safeGit = [
                    "--no-replace-objects",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    `--git-dir=${path.join(correctionRuntime.session.workspace, ".git")}`,
                    `--work-tree=${correctionRuntime.session.workspace}`,
                  ];
                  const configuredFilters = spawnSync(
                    "git",
                    [
                      ...safeGit,
                      "-C",
                      correctionRuntime.session.workspace,
                      "config",
                      "--null",
                      "--name-only",
                      "--get-regexp",
                      "^filter\\..*\\.(clean|process)$",
                    ],
                    { encoding: "buffer", env: verifiedGitEnv },
                  );
                  if (
                    configuredFilters.error ||
                    ![0, 1].includes(configuredFilters.status ?? -1)
                  )
                    throw new Error(
                      "cannot inspect correction checkout filter commands",
                    );
                  if (configuredFilters.status === 0) {
                    if (configuredFilters.stdout.at(-1) !== 0)
                      throw new Error(
                        "incomplete correction checkout filter configuration",
                      );
                    const filters = new TextDecoder("utf-8", {
                      fatal: true,
                    }).decode(configuredFilters.stdout.subarray(0, -1));
                    for (const name of new Set(filters.split("\0")))
                      safeGit.push("-c", `${name}=`);
                  }
                  const head = spawnSync(
                    "git",
                    [
                      ...safeGit,
                      "-C",
                      correctionRuntime.session.workspace,
                      "rev-parse",
                      "HEAD",
                    ],
                    { encoding: "utf8", timeout: 10_000, env: verifiedGitEnv },
                  );
                  const status = spawnSync(
                    "git",
                    [
                      ...safeGit,
                      "-C",
                      correctionRuntime.session.workspace,
                      "status",
                      "--porcelain",
                      "--untracked-files=all",
                    ],
                    { encoding: "utf8", timeout: 10_000, env: verifiedGitEnv },
                  );
                  const ancestry = spawnSync(
                    "git",
                    [
                      ...safeGit,
                      "-C",
                      correctionRuntime.session.workspace,
                      "merge-base",
                      "--is-ancestor",
                      correctionRuntime.baseSha,
                      commitSha,
                    ],
                    { encoding: "utf8", timeout: 10_000, env: verifiedGitEnv },
                  );
                  const diff = spawnSync(
                    "git",
                    [
                      ...safeGit,
                      "-C",
                      correctionRuntime.session.workspace,
                      "diff",
                      "--no-renames",
                      "--no-ext-diff",
                      "--no-textconv",
                      "--name-only",
                      "-z",
                      "--diff-filter=ACDMRT",
                      `${baseSha}..${commitSha}`,
                    ],
                    {
                      encoding: "buffer",
                      timeout: 10_000,
                      env: verifiedGitEnv,
                    },
                  );
                  if (
                    head.status !== 0 ||
                    head.stdout.trim().toLowerCase() !==
                      commitSha.toLowerCase() ||
                    status.status !== 0 ||
                    status.stdout.trim() ||
                    diff.status !== 0 ||
                    ancestry.status !== 0 ||
                    (diff.stdout.length > 0 && diff.stdout.at(-1) !== 0)
                  )
                    throw new Error(
                      "Developer correction candidate is not a clean HEAD of its assignment workspace",
                    );
                  const actualScope = diff.stdout.length
                    ? new TextDecoder("utf-8", { fatal: true, ignoreBOM: true })
                        .decode(diff.stdout.subarray(0, -1))
                        .split("\0")
                        .sort()
                    : [];
                  const reportedScope = [...(changedScope as string[])].sort();
                  if (
                    !actualScope.length ||
                    actualScope.length !== reportedScope.length ||
                    actualScope.some(
                      (file, index) => file !== reportedScope[index],
                    ) ||
                    actualScope.some(
                      (file) =>
                        file.startsWith("/") ||
                        file.split(/[\\/]/).includes("..") ||
                        !slice.writeScope.some(
                          (scope) =>
                            file === scope ||
                            file.startsWith(`${scope.replace(/\/+$/, "")}/`),
                        ),
                    )
                  )
                    throw new Error(
                      "Developer correction changes files outside its validated scope",
                    );
                  const candidate: CandidateInput = {
                    candidateId,
                    assignmentId: correctionAssignment.assignmentId,
                    commitSha,
                    baseSha: correctionRuntime.baseSha,
                    changedScope: reportedScope,
                    limitations: limitations as string[],
                    evidence: developerEvidence as string[],
                  };
                  core.submitCandidate(
                    context(core, identities.Developer.credential),
                    candidate,
                  );
                  candidateWorkspaces[candidateId] =
                    correctionRuntime.session.workspace;
                  candidateUnderReviewByWorkItem.set(
                    correctionWorkItemId,
                    candidateId,
                  );
                  const candidateCheckpointBlocker = await evaluateSupervisor();
                  if (candidateCheckpointBlocker)
                    return `Supervisor correction candidate remains unaccepted: ${candidateCheckpointBlocker}`;
                  if (
                    dispatches >= plan.limits.maxDispatches ||
                    Date.now() >= findingDeadlineMs
                  )
                    throw new Error(
                      "correction Verifier dispatch exceeded its bounded budget",
                    );
                  const verifierWorkItemId = `verify-${randomUUID()}`;
                  core.createWorkItem(context(core, credential), {
                    workItemId: verifierWorkItemId,
                    title: `Verify correction candidate ${candidateId}`,
                    description: `Independently verify candidate ${candidateId} at exactly commit ${commitSha}, startingSha ${commitSha}, against every criterion: ${slice.acceptanceCriteria.join("; ")}. Do not modify source files. Return JSON {candidateId,commitSha,startingSha,evidence} with one {criterion,passed,observation,exitStatus,artifactRef} item per criterion.`,
                    requiredRole: "Verifier",
                    parentWorkItemId: correctionWorkItemId,
                    acceptanceCriteria: slice.acceptanceCriteria,
                  });
                  const dispatchReadinessBlocker = await evaluateSupervisor();
                  if (dispatchReadinessBlocker)
                    return `Supervisor correction Verifier dispatch remains blocked: ${dispatchReadinessBlocker}`;
                  if (Date.now() >= findingDeadlineMs)
                    throw new Error(
                      "correction Verifier dispatch exceeded its bounded budget",
                    );
                  dispatches = reserveDispatchSlot(
                    dispatches,
                    plan.limits.maxDispatches,
                  );
                  core.markReady(context(core, credential), verifierWorkItemId);
                  const verifierAssignment = core.assignWorkItem(
                    context(core, credential),
                    verifierWorkItemId,
                    identities.Verifier.seatId,
                    candidateId,
                  );
                  const verifierRuntime = await provisionRuntime(
                    "Verifier",
                    identities.Verifier.seatId,
                    verifierWorkItemId,
                    verifierAssignment.generation,
                    verifierAssignment.assignmentId,
                    [],
                    {
                      workspace: correctionRuntime.session.workspace,
                      commitSha,
                    },
                  );
                  runtimeCommands[verifierRuntime.session.sessionId] =
                    verifierAssignment.commandId;
                  let verifierContained = false;
                  try {
                    await waitForDispatchPermission();
                    core.transitionRuntimeSession(
                      context(core, credential),
                      verifierRuntime.session.sessionId,
                      "working",
                    );
                    await verifierRuntime.adapter.dispatchAndStart(
                      context(core, credential),
                      verifierAssignment.commandId,
                    );
                    const report = await waitForReport(
                      verifierWorkItemId,
                      verifierAssignment.assignmentId,
                      Math.min(deadlineMs, findingDeadlineMs),
                    );
                    if (
                      !report ||
                      report.assignmentId !== verifierAssignment.assignmentId ||
                      report.role !== "Verifier" ||
                      report.inputRevision !== core.inputRevision
                    )
                      throw new Error(
                        "correction Verifier report is missing or mismatched",
                      );
                    await containRuntime(
                      verifierRuntime.session,
                      verifierAssignment.assignmentId,
                    );
                    verifierContained = true;
                    const verifiedHead = spawnSync(
                      "git",
                      [
                        "--no-replace-objects",
                        "-c",
                        "core.fsmonitor=false",
                        "-c",
                        "core.hooksPath=/dev/null",
                        `--git-dir=${path.join(verifierRuntime.session.workspace, ".git")}`,
                        `--work-tree=${verifierRuntime.session.workspace}`,
                        "-C",
                        verifierRuntime.session.workspace,
                        "rev-parse",
                        "HEAD",
                      ],
                      {
                        encoding: "utf8",
                        timeout: 10_000,
                        env: verifiedGitEnv,
                      },
                    );
                    const reply = objectRecord(
                      parseJsonWithoutDuplicateMembers(report.reply),
                    );
                    if (
                      verifiedHead.status !== 0 ||
                      verifiedHead.stdout.trim().toLowerCase() !==
                        commitSha.toLowerCase() ||
                      verifierRuntime.baseSha.toLowerCase() !==
                        commitSha.toLowerCase() ||
                      reply?.candidateId !== candidateId ||
                      reply.commitSha !== commitSha ||
                      reply.startingSha !== commitSha ||
                      !Array.isArray(reply.evidence) ||
                      reply.evidence.length !== slice.acceptanceCriteria.length
                    )
                      throw new Error(
                        "correction Verifier report does not bind the exact candidate",
                      );
                    assertTrackedCheckoutMatchesHead(
                      verifierRuntime.session.workspace,
                      commitSha,
                    );
                    const seen = new Set<string>();
                    const evidence: EvidenceInput[] = [];
                    if (!verifierRuntime.evidenceDirectory)
                      throw new Error(
                        "correction Verifier evidence directory is unavailable",
                      );
                    for (const raw of reply.evidence) {
                      const entry = objectRecord(raw);
                      if (
                        typeof entry?.criterion !== "string" ||
                        !slice.acceptanceCriteria.includes(entry.criterion) ||
                        seen.has(entry.criterion) ||
                        entry.passed !== true ||
                        typeof entry.observation !== "string" ||
                        !entry.observation.trim() ||
                        entry.exitStatus !== 0 ||
                        typeof entry.artifactRef !== "string" ||
                        !entry.artifactRef.startsWith("/evidence/")
                      )
                        throw new Error(
                          "correction Verifier evidence must pass each exact criterion",
                        );
                      seen.add(entry.criterion);
                      evidence.push({
                        evidenceId: randomUUID(),
                        candidateId,
                        criterion: entry.criterion,
                        passed: true,
                        observation: entry.observation,
                        exitStatus: 0,
                        artifactRef: verifiedEvidencePath(
                          verifierRuntime.evidenceDirectory,
                          entry.artifactRef,
                        ),
                      });
                    }
                    if (seen.size !== slice.acceptanceCriteria.length)
                      throw new Error(
                        "correction Verifier omitted an acceptance criterion",
                      );
                    core.recordEvidenceBatch(
                      context(core, identities.Verifier.credential),
                      verifierAssignment.assignmentId,
                      evidence,
                    );
                  } catch (error) {
                    candidateUnderReviewByWorkItem.delete(correctionWorkItemId);
                    throw error;
                  } finally {
                    if (!verifierContained)
                      await containRuntime(
                        verifierRuntime.session,
                        verifierAssignment.assignmentId,
                      );
                  }
                  const correctionReviewBlocker = await evaluateSupervisor();
                  if (correctionReviewBlocker)
                    return `Supervisor correction report remains unaccepted: ${correctionReviewBlocker}`;
                  core.acceptCandidate(
                    context(core, credential),
                    correctionWorkItemId,
                    candidateId,
                  );
                  const verificationBlocker = await evaluateSupervisor();
                  if (verificationBlocker)
                    return `Supervisor correction requires further review: ${verificationBlocker}`;
                  const verifiedCondition =
                    supervisorVerifiedFindings.get(findingId);
                  const finding = core
                    .statusSnapshot()
                    .findings.find((entry) => entry.findingId === findingId);
                  const correctionEvidence =
                    core.latestAcceptedWorkEvent(correctionWorkItemId);
                  if (!verifiedCondition || !finding || !correctionEvidence)
                    return `Supervisor did not verify the recorded condition for finding ${findingId}`;
                  acceptedCandidateByWorkItem.set(
                    affected.workItemId,
                    candidateId,
                  );
                  candidateUnderReviewByWorkItem.delete(correctionWorkItemId);
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "resolved",
                    {
                      condition: finding.resolutionCondition,
                      evidenceEventId: correctionEvidence.eventId,
                      assignmentId: correctionAssignment.assignmentId,
                      generation: correctionAssignment.generation,
                      evidence: {
                        supervisorCheckpointAssignmentId:
                          verifiedCondition.assignmentId,
                        supervisorVerificationAssignmentId:
                          verifiedCondition.assignmentId,
                        correctionEventId: correctionEvidence.eventId,
                        condition: finding.resolutionCondition,
                        supervisorVerificationEvidence:
                          verifiedCondition.evidence,
                      },
                    },
                  );
                  const finalReviewBlocker = await evaluateSupervisor();
                  if (finalReviewBlocker)
                    return `Resolved finding requires a fresh checkpoint: ${finalReviewBlocker}`;
                  if (
                    !core
                      .statusSnapshot()
                      .findings.some((entry) => entry.state !== "resolved")
                  )
                    supervisorMustPause = false;
                  return undefined;
                }
                if (
                  correctionReply.disposition === "correcting" &&
                  affected.role === "PM"
                ) {
                  const correctionReviewBlocker = await evaluateSupervisor();
                  if (correctionReviewBlocker)
                    return `Supervisor correction report remains unaccepted: ${correctionReviewBlocker}`;
                  core.acceptNonCandidateReport(
                    context(core, credential),
                    correctionWorkItemId,
                    correctionAssignment.assignmentId,
                  );
                  const verificationBlocker = await evaluateSupervisor();
                  if (verificationBlocker)
                    return `Supervisor correction requires further review: ${verificationBlocker}`;
                  const verifiedCondition =
                    supervisorVerifiedFindings.get(findingId);
                  if (!verifiedCondition)
                    return `Supervisor did not verify the recorded condition for finding ${findingId}`;
                  const finding = core
                    .statusSnapshot()
                    .findings.find((entry) => entry.findingId === findingId);
                  const correctionEvidence =
                    core.latestAcceptedWorkEvent(correctionWorkItemId);
                  if (!finding || !correctionEvidence)
                    throw new Error(
                      "Supervisor could not bind correction resolution evidence",
                    );
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "resolved",
                    {
                      condition: finding.resolutionCondition,
                      evidenceEventId: correctionEvidence.eventId,
                      assignmentId: correctionAssignment.assignmentId,
                      generation: correctionAssignment.generation,
                      evidence: {
                        supervisorCheckpointAssignmentId:
                          verifiedCondition.assignmentId,
                        supervisorVerificationAssignmentId:
                          verifiedCondition.assignmentId,
                        correctionEventId: correctionEvidence.eventId,
                        condition: finding.resolutionCondition,
                        supervisorVerificationEvidence:
                          verifiedCondition.evidence,
                      },
                    },
                  );
                  const finalReviewBlocker = await evaluateSupervisor();
                  if (finalReviewBlocker)
                    return `Resolved finding requires a fresh checkpoint: ${finalReviewBlocker}`;
                  if (
                    !core
                      .statusSnapshot()
                      .findings.some((entry) => entry.state !== "resolved")
                  )
                    supervisorMustPause = false;
                  return undefined;
                }
                return `Supervisor finding ${findingId} was acknowledged; correction status is ${correctionReply.disposition}: ${correctionReply.response}`;
              } catch (correctionError) {
                const correctionReason =
                  correctionError instanceof Error
                    ? correctionError.message
                    : "correction handoff failed";
                if (
                  correctionRuntime &&
                  correctionAssignmentId &&
                  !correctionContained
                ) {
                  try {
                    await containRuntime(
                      correctionRuntime.session,
                      correctionAssignmentId,
                    );
                  } catch {
                    return `Supervisor finding recorded; correction failed and authority containment is unproven: ${correctionReason}`;
                  }
                }
                const currentFinding = core
                  .statusSnapshot()
                  .findings.find((entry) => entry.findingId === findingId);
                if (
                  currentFinding?.state === "reported" &&
                  Date.now() >= findingDeadlineMs
                ) {
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "escalated",
                    { reason: "acknowledgement deadline expired" },
                  );
                  return `Supervisor finding ${findingId} escalated after its acknowledgement deadline: ${correctionReason}`;
                }
                if (
                  currentFinding &&
                  currentFinding.state !== "resolved" &&
                  currentFinding.state !== "escalated"
                ) {
                  core.transitionFinding(
                    context(core, credential),
                    findingId,
                    "escalated",
                    {
                      reason: `contained correction handoff failed; operator review required: ${correctionReason}`,
                    },
                  );
                  supervisorMustPause = true;
                  return `Supervisor finding ${findingId} escalated to the operator after correction handoff failure: ${correctionReason}`;
                }
                return `Supervisor finding recorded; correction handoff failed: ${correctionReason}`;
              }
            }
            return undefined;
          } catch (error) {
            const reason =
              error instanceof Error
                ? error.message
                : "Supervisor evaluation failed";
            try {
              core.markSupervisionDegraded(context(core, credential), reason);
            } catch {
              // The persisted degraded state is already fail-safe if this retry fails.
            }
            if (runtime && assignmentId && !containmentProven) {
              try {
                await containRuntime(runtime.session, assignmentId);
                containmentProven = true;
              } catch {
                supervisorMustPause = true;
                return `Supervisor unavailable: ${reason}; authority containment is unproven`;
              }
            }
            if (assignmentId && containmentProven) {
              try {
                const failedSupervisorWork = supervisorWorkItemId
                  ? core
                      .statusSnapshot()
                      .work.find(
                        (work) => work.workItemId === supervisorWorkItemId,
                      )
                  : undefined;
                if (
                  supervisorWorkItemId &&
                  assignmentId &&
                  failedSupervisorWork &&
                  ["blocked", "awaiting_verification"].includes(
                    failedSupervisorWork.state,
                  )
                )
                  core.cancelSupervisorReport(
                    context(core, credential),
                    supervisorWorkItemId,
                    assignmentId,
                    "Supervisor evaluation failed after containment",
                  );
                core.claimSupervisorReplacement(context(core, credential));
                return await evaluateSupervisor();
              } catch {
                // The one durable replacement budget is exhausted or unavailable.
              }
            }
            supervisorMustPause = true;
            return `Supervisor unavailable: ${reason}`;
          }
        };
        const stepScheduler = async (): Promise<SchedulerStep> => {
          if (stopping || Date.now() >= deadlineMs)
            return {
              state: "stopped",
              reason: stopping
                ? "run canceled by signal"
                : "maxRunMs exceeded before slice dispatch",
            };
          await waitForDispatchPermission();
          if (dispatches >= plan.limits.maxDispatches) {
            const work = core.statusSnapshot().work;
            return plan.slices.every((slice) =>
              work.some(
                (item) =>
                  item.workItemId ===
                    workflowWorkItemId(plan.taskId, slice.id) &&
                  item.state === "accepted",
              ),
            )
              ? { state: "complete" }
              : {
                  state: "stopped",
                  reason: "maxDispatches exhausted before slice dispatch",
                };
          }
          const planWorkIds = new Set(
            plan.slices.map((slice) =>
              workflowWorkItemId(plan.taskId, slice.id),
            ),
          );
          const blockedWork = core
            .statusSnapshot()
            .work.find(
              (work) =>
                planWorkIds.has(work.workItemId) &&
                (work.state === "blocked" || work.state === "ready"),
            );
          if (!blockedWork) return scheduler.step();
          const previous = core.latestAssignmentForWorkItem(
            blockedWork.workItemId,
          );
          if (!previous) return scheduler.step();
          if (previous.authorityState !== "contained")
            return {
              state: "stopped",
              reason: "latest Developer assignment authority is not contained",
            };
          let recoveryId = core.pendingReplacementRecovery(
            blockedWork.workItemId,
            previous.assignmentId,
          );
          if (!recoveryId) {
            const recovery = core.recordRecovery(context(core, credential), {
              workItemId: blockedWork.workItemId,
              assignmentId: previous.assignmentId,
              recoveryId: randomUUID(),
              recoveryType: "worker_replacement",
              reason: "restart after verified prior-session containment",
            });
            if (recovery.outcome !== "pending") return scheduler.step();
            recoveryId = recovery.recoveryId;
          }
          return scheduler.step({
            workItemId: blockedWork.workItemId,
            recoveryId,
          });
        };
        if (!blocker && planWasAccepted) blocker = await evaluateSupervisor();
        let step = await stepScheduler();
        while (
          !stopping &&
          !blocker &&
          step.state !== "complete" &&
          step.state !== "stopped" &&
          Date.now() < deadlineMs
        ) {
          if (step.state !== "dispatched") {
            blocker =
              step.state === "waiting_for_plan_acceptance"
                ? "the PM plan report has not been accepted by ControllerCore"
                : `work ${step.workItemId} is waiting: ${step.reasons.join("; ")}`;
            break;
          }
          const dispatchStep = step;
          const assignment = dispatchStep.assignment;
          const workItemId = assignment.workItemId;
          const slice = plan.slices.find(
            (entry) => entry.id === dispatchStep.sliceId,
          );
          const developerSession = sessions[identities.Developer.seatId];
          const developerReport = await waitForReport(
            workItemId,
            assignment.assignmentId,
          );
          if (!developerReport) {
            blocker = `Developer report for ${dispatchStep.sliceId} did not complete before the bounded deadline`;
            break;
          }
          if (
            !slice ||
            !developerSession ||
            developerReport.assignmentId !== assignment.assignmentId ||
            developerReport.role !== "Developer" ||
            developerReport.inputRevision !== core.inputRevision
          )
            throw new Error(
              `Developer report for ${dispatchStep.sliceId} does not match its active assignment`,
            );
          await containRuntime(developerSession, assignment.assignmentId);
          const developerMetadata =
            workspaceMetadata[developerSession.sessionId];
          if (!developerMetadata)
            throw new Error(
              "Developer assignment workspace base was not recorded",
            );
          const candidateReply = objectRecord(
            parseJsonWithoutDuplicateMembers(developerReport.reply),
          );
          const candidateId = candidateReply?.candidateId;
          const commitSha = candidateReply?.commitSha;
          const reportedBaseSha = candidateReply?.baseSha;
          const changedScope = candidateReply?.changedScope;
          const limitations = candidateReply?.limitations;
          const developerEvidence = candidateReply?.evidence;
          if (
            typeof candidateId !== "string" ||
            candidateId !== `candidate-${assignment.assignmentId}` ||
            typeof commitSha !== "string" ||
            !/^[a-f0-9]{40}([a-f0-9]{24})?$/i.test(commitSha) ||
            typeof reportedBaseSha !== "string" ||
            reportedBaseSha !== developerMetadata.baseSha ||
            !Array.isArray(changedScope) ||
            changedScope.some((entry) => typeof entry !== "string") ||
            !Array.isArray(limitations) ||
            limitations.some((entry) => typeof entry !== "string") ||
            !Array.isArray(developerEvidence) ||
            developerEvidence.length === 0 ||
            developerEvidence.length > 64 ||
            developerEvidence.some(
              (entry) =>
                typeof entry !== "string" ||
                entry.trim().length === 0 ||
                entry.length > 4096,
            )
          )
            throw new Error(
              `Developer report for ${dispatchStep.sliceId} lacks a valid immutable candidate identity`,
            );
          // The checkout belongs to the worker; host Git must not run its configured commands.
          const safeGit = [
            "--no-replace-objects",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            `--git-dir=${path.join(developerMetadata.workspace, ".git")}`,
            `--work-tree=${developerMetadata.workspace}`,
          ];
          const configuredFilters = spawnSync(
            "git",
            [
              ...safeGit,
              "-C",
              developerMetadata.workspace,
              "config",
              "--null",
              "--name-only",
              "--get-regexp",
              "^filter\\..*\\.(clean|process)$",
            ],
            { encoding: "buffer", env: verifiedGitEnv },
          );
          if (
            configuredFilters.error ||
            ![0, 1].includes(configuredFilters.status ?? -1)
          )
            throw new Error(
              "cannot inspect Developer checkout filter commands",
            );
          if (configuredFilters.status === 0) {
            if (configuredFilters.stdout.at(-1) !== 0)
              throw new Error(
                "incomplete Developer checkout filter configuration",
              );
            const filters = new TextDecoder("utf-8", { fatal: true }).decode(
              configuredFilters.stdout.subarray(0, -1),
            );
            for (const name of new Set(filters.split("\0")))
              safeGit.push("-c", `${name}=`);
          }
          const workspaceHead = spawnSync(
            "git",
            [
              ...safeGit,
              "-C",
              developerMetadata.workspace,
              "rev-parse",
              "HEAD",
            ],
            { encoding: "utf8", env: verifiedGitEnv },
          );
          const workspaceStatus = spawnSync(
            "git",
            [
              ...safeGit,
              "-C",
              developerMetadata.workspace,
              "status",
              "--porcelain",
              "--untracked-files=all",
            ],
            { encoding: "utf8", env: verifiedGitEnv },
          );
          if (
            workspaceHead.status !== 0 ||
            workspaceHead.stdout.trim().toLowerCase() !==
              commitSha.toLowerCase() ||
            workspaceStatus.status !== 0 ||
            workspaceStatus.stdout.trim() !== ""
          )
            throw new Error(
              "Developer candidate commit is not the clean HEAD of its isolated assignment checkout",
            );
          const ancestry = spawnSync(
            "git",
            [
              ...safeGit,
              "-C",
              developerMetadata.workspace,
              "merge-base",
              "--is-ancestor",
              developerMetadata.baseSha,
              commitSha,
            ],
            { encoding: "utf8", env: verifiedGitEnv },
          );
          if (ancestry.status !== 0)
            throw new Error(
              `Developer candidate is not descended from its recorded base: ${(ancestry.stderr || "base ancestry check failed").trim()}`,
            );
          for (const dependency of slice.dependsOn) {
            const dependencyWorkId = workflowWorkItemId(
              plan.taskId,
              dependency,
            );
            const dependencyWork = objectRecord(
              objectRecord(core.inspect(dependencyWorkId))?.record,
            );
            const dependencyCandidateId = dependencyWork?.accepted_candidate_id;
            const dependencyRecord =
              typeof dependencyCandidateId === "string"
                ? objectRecord(
                    objectRecord(core.inspect(dependencyCandidateId))?.record,
                  )
                : undefined;
            if (
              typeof dependencyRecord?.commit_sha !== "string" ||
              typeof dependencyRecord.base_sha !== "string"
            )
              throw new Error(
                `accepted dependency ${dependency} lacks immutable candidate ancestry`,
              );
            const dependencyAncestor = spawnSync(
              "git",
              [
                ...safeGit,
                "-C",
                developerMetadata.workspace,
                "merge-base",
                "--is-ancestor",
                dependencyRecord.commit_sha,
                developerMetadata.baseSha,
              ],
              { encoding: "utf8", env: verifiedGitEnv },
            );
            if (dependencyAncestor.status !== 0)
              throw new Error(
                `candidate base does not contain accepted dependency ${dependency} at ${dependencyRecord.commit_sha}`,
              );
          }
          const diff = spawnSync(
            "git",
            [
              ...safeGit,
              "-C",
              developerMetadata.workspace,
              "diff",
              "--no-renames",
              "--no-ext-diff",
              "--no-textconv",
              "--name-only",
              "-z",
              "--diff-filter=ACDMRT",
              `${developerMetadata.baseSha}..${commitSha}`,
            ],
            { encoding: "buffer", env: verifiedGitEnv },
          );
          if (diff.status !== 0)
            throw new Error(
              `cannot inspect Developer candidate diff: ${(diff.stderr?.toString("utf8") || "git diff failed").trim()}`,
            );
          if (diff.stdout.length && diff.stdout.at(-1) !== 0)
            throw new Error(
              "Developer candidate diff has an incomplete filename frame",
            );
          const actualScope = diff.stdout.length
            ? new TextDecoder("utf-8", { fatal: true, ignoreBOM: true })
                .decode(diff.stdout.subarray(0, -1))
                .split("\0")
                .sort()
            : [];
          const reportedScope = [...(changedScope as string[])].sort();
          if (
            actualScope.length === 0 ||
            actualScope.length !== reportedScope.length ||
            actualScope.some((file, index) => file !== reportedScope[index]) ||
            actualScope.some(
              (file) =>
                file.startsWith("/") ||
                file.split(/[\\/]/).includes("..") ||
                !slice.writeScope.some(
                  (scope) =>
                    file === scope ||
                    file.startsWith(`${scope.replace(/\/+$/, "")}/`),
                ),
            )
          )
            throw new Error(
              `Developer candidate ${candidateId} changes files outside its validated scope`,
            );
          const candidate: CandidateInput = {
            candidateId,
            assignmentId: assignment.assignmentId,
            commitSha,
            baseSha: developerMetadata.baseSha,
            changedScope: reportedScope,
            limitations: limitations as string[],
            evidence: developerEvidence as string[],
          };
          core.submitCandidate(
            context(core, identities.Developer.credential),
            candidate,
          );
          candidateWorkspaces[candidateId] = developerMetadata.workspace;
          candidateUnderReviewByWorkItem.set(workItemId, candidateId);
          if (stopping) {
            blocker = "run canceled by signal";
            step = await stepScheduler();
            break;
          }
          if (dispatches >= plan.limits.maxDispatches) {
            blocker = "maxDispatches reached before Verifier evidence";
            step = await stepScheduler();
            break;
          }
          const verifierWorkItemId = `verify-${randomUUID()}`;
          core.createWorkItem(context(core, credential), {
            workItemId: verifierWorkItemId,
            title: `Verify ${dispatchStep.sliceId}: ${candidateId}`,
            description: `Independently verify candidate ${candidateId} at exactly commit ${commitSha}, startingSha ${commitSha}, against every slice acceptance criterion: ${slice.acceptanceCriteria.join("; ")}. Use the separate read-only workspace; verify \`git rev-parse HEAD\` equals ${commitSha} before checks and remains unchanged. Do not modify source files. Return JSON {candidateId,commitSha,startingSha,evidence}, with exactly one evidence item per criterion: {criterion,passed,observation,exitStatus,artifactRef}. observation must be a non-empty concise description of what was checked. exitStatus must be the actual 0–255 exit code from the criterion check. Persist each artifact beneath /evidence; artifactRef must be an absolute /evidence/... path, and the artifact must contain the command, exit status, stdout/stderr, and observation. Do not claim a pass without evidence.`,
            requiredRole: "Verifier",
            parentWorkItemId: workItemId,
          });
          blocker = await evaluateSupervisor();
          if (blocker) break;
          core.markReady(context(core, credential), verifierWorkItemId);
          const verifierAssignment = core.assignWorkItem(
            context(core, credential),
            verifierWorkItemId,
            identities.Verifier.seatId,
            candidateId,
          );
          const verifierRuntime = await provisionRuntime(
            "Verifier",
            identities.Verifier.seatId,
            verifierWorkItemId,
            verifierAssignment.generation,
            verifierAssignment.assignmentId,
            [],
            { workspace: developerMetadata.workspace, commitSha },
          );
          runtimeCommands[verifierRuntime.session.sessionId] =
            verifierAssignment.commandId;
          await waitForDispatchPermission();
          dispatches += 1;
          core.transitionRuntimeSession(
            context(core, credential),
            verifierRuntime.session.sessionId,
            "working",
          );
          await verifierRuntime.adapter.dispatchAndStart(
            context(core, credential),
            verifierAssignment.commandId,
          );
          const verifierReport = await waitForReport(
            verifierWorkItemId,
            verifierAssignment.assignmentId,
          );
          if (!verifierReport) {
            blocker = `Verifier report for candidate ${candidateId} did not complete before the bounded deadline`;
            step = await stepScheduler();
            break;
          }
          if (
            verifierReport.assignmentId !== verifierAssignment.assignmentId ||
            verifierReport.role !== "Verifier" ||
            verifierReport.inputRevision !== core.inputRevision
          )
            throw new Error(
              "Verifier report does not match its candidate-bound assignment",
            );
          await containRuntime(
            verifierRuntime.session,
            verifierAssignment.assignmentId,
          );
          const verifierSafeGit = [
            "--no-replace-objects",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            `--git-dir=${path.join(verifierRuntime.session.workspace, ".git")}`,
            `--work-tree=${verifierRuntime.session.workspace}`,
          ];
          const verifierHead = spawnSync(
            "git",
            [
              ...verifierSafeGit,
              "-C",
              verifierRuntime.session.workspace,
              "rev-parse",
              "HEAD",
            ],
            { encoding: "utf8", timeout: 10_000, env: verifiedGitEnv },
          );
          if (
            verifierHead.status !== 0 ||
            verifierHead.stdout.trim().toLowerCase() !==
              commitSha.toLowerCase() ||
            verifierRuntime.baseSha.toLowerCase() !== commitSha.toLowerCase()
          )
            throw new Error(
              "Verifier changed or did not inspect the exact immutable candidate checkout",
            );
          assertTrackedCheckoutMatchesHead(
            verifierRuntime.session.workspace,
            commitSha,
          );
          const verifierReply = objectRecord(
            parseJsonWithoutDuplicateMembers(verifierReport.reply),
          );
          if (
            verifierReply?.candidateId !== candidateId ||
            verifierReply.commitSha !== commitSha ||
            verifierReply.startingSha !== verifierRuntime.baseSha ||
            !Array.isArray(verifierReply.evidence) ||
            verifierReply.evidence.length !== slice.acceptanceCriteria.length
          )
            throw new Error(
              "Verifier report does not bind the exact candidate checkout and every slice criterion",
            );
          const seenCriteria = new Set<string>();
          const evidence: EvidenceInput[] = [];
          const evidenceDirectory = verifierRuntime.evidenceDirectory;
          for (const rawEvidence of verifierReply.evidence) {
            const entry = objectRecord(rawEvidence);
            const criterion = entry?.criterion;
            const artifactRef = entry?.artifactRef;
            if (
              typeof criterion !== "string" ||
              !slice.acceptanceCriteria.includes(criterion) ||
              seenCriteria.has(criterion) ||
              typeof entry?.passed !== "boolean" ||
              typeof entry.observation !== "string" ||
              !entry.observation.trim() ||
              !Number.isInteger(entry.exitStatus) ||
              (entry.exitStatus as number) < 0 ||
              (entry.exitStatus as number) > 255 ||
              (entry.passed === true && entry.exitStatus !== 0) ||
              typeof artifactRef !== "string" ||
              !artifactRef.startsWith("/evidence/")
            )
              throw new Error(
                "Verifier evidence must include the exact criterion, observation, exit status, and artifact",
              );
            seenCriteria.add(criterion);
            const artifactRealPath = verifiedEvidencePath(
              evidenceDirectory,
              artifactRef,
            );
            evidence.push({
              evidenceId: randomUUID(),
              candidateId,
              criterion,
              passed: entry.passed,
              observation: entry.observation,
              exitStatus: entry.exitStatus as number,
              artifactRef: artifactRealPath,
            });
          }
          if (
            slice.acceptanceCriteria.some(
              (criterion) => !seenCriteria.has(criterion),
            )
          )
            throw new Error("Verifier omitted a slice acceptance criterion");
          core.recordEvidenceBatch(
            context(core, identities.Verifier.credential),
            verifierAssignment.assignmentId,
            evidence,
          );
          const failed = evidence.find((item) => !item.passed);
          if (failed) {
            candidateUnderReviewByWorkItem.delete(workItemId);
            const failedObservation = verifierReply.evidence.find(
              (item) => objectRecord(item)?.criterion === failed.criterion,
            );
            const observation = objectRecord(failedObservation)?.observation;
            if (
              dispatches + 2 > plan.limits.maxDispatches ||
              Date.now() >= deadlineMs
            ) {
              blocker = `Verifier rejected candidate ${candidateId} for ${failed.criterion}; no bounded remediation budget remains`;
              step = await stepScheduler();
              break;
            }
            const recoveryId = randomUUID();
            const recovery = core.recordRecovery(context(core, credential), {
              recoveryId,
              workItemId,
              assignmentId: assignment.assignmentId,
              recoveryType: "implementation_remediation",
              reason: `Verifier rejected candidate ${candidateId}; criterion=${failed.criterion}; observation=${String(observation)}; artifact=${failed.artifactRef}`,
            });
            if (recovery.outcome !== "pending") {
              blocker = `Verifier rejected candidate ${candidateId}; bounded remediation limit ${recovery.limit} reached`;
              step = await stepScheduler();
              break;
            }
            blocker = await evaluateSupervisor();
            if (blocker) break;
            try {
              await waitForDispatchPermission();
            } catch (error) {
              blocker = `Verifier rejected candidate ${candidateId}; replacement was not dispatched: ${error instanceof Error ? error.message : String(error)}`;
              break;
            }
            step = await scheduler.step({ workItemId, recoveryId });
            if (step.state !== "dispatched") {
              blocker = `Verifier rejected candidate ${candidateId}; replacement was blocked: ${step.state === "waiting" ? step.reasons.join("; ") : step.state === "stopped" ? step.reason : "scheduler did not dispatch"}`;
              break;
            }
            continue;
          }
          blocker = await evaluateSupervisor();
          if (blocker) break;
          core.acceptCandidate(
            context(core, credential),
            workItemId,
            candidateId,
          );
          acceptedCandidateByWorkItem.set(workItemId, candidateId);
          candidateUnderReviewByWorkItem.delete(workItemId);
          blocker = await evaluateSupervisor();
          if (blocker) break;
          step = await stepScheduler();
        }
        if (
          !stopping &&
          !blocker &&
          planWasAccepted &&
          step.state === "complete"
        ) {
          const acceptedSnapshot = core.statusSnapshot();
          const acceptedSlices = validateWorkflowPlan(plan).order.map(
            (sliceId) => {
              const slice = plan.slices.find((entry) => entry.id === sliceId);
              if (!slice)
                throw new Error(
                  `validated plan order references missing slice ${sliceId}`,
                );
              const workItemId = `wf-${createHash("sha256").update(`${plan.taskId}:${slice.id}`).digest("hex").slice(0, 24)}`;
              const candidateId = acceptedCandidateByWorkItem.get(workItemId);
              const candidate = acceptedSnapshot.evidence.find(
                (entry) => entry.candidateId === candidateId,
              );
              const workspace = candidateId
                ? candidateWorkspaces[candidateId]
                : undefined;
              if (
                !candidate ||
                !workspace ||
                !acceptedSnapshot.work.some(
                  (work) =>
                    work.workItemId === workItemId && work.state === "accepted",
                )
              )
                throw new Error(
                  `accepted slice ${slice.id} has no contained candidate to supervise`,
                );
              return {
                candidateId,
                workItemId,
                workspace,
                commitSha: candidate.commitSha,
              };
            },
          );
          let finalRuntime:
            Awaited<ReturnType<typeof provisionRuntime>> | undefined;
          const finalWorkItemId = `final-${randomUUID()}`;
          if (Date.now() >= deadlineMs)
            blocker = "maxRunMs exceeded before final-parent verification";
          else if (dispatches >= plan.limits.maxDispatches)
            blocker =
              "maxDispatches exhausted before final-parent verification";
          else {
            core.createWorkItem(context(core, credential), {
              workItemId: finalWorkItemId,
              title: `Verify final parent acceptance ${plan.taskId}`,
              description: `Independently verify the composed checkout at /workspace, starting from the exact composed commit ${finalRuntime?.baseSha ?? "provided at dispatch"}, against every parent acceptance criterion: ${plan.acceptanceCriteria.join("; ")}. Verify git rev-parse HEAD before and after checks; do not modify source files. Return JSON {"commitSha":"full git HEAD","startingSha":"full starting HEAD","evidence":[{"criterion":"exact listed criterion","passed":true|false,"observation":"what was checked","exitStatus":0,"artifactRef":"/evidence/..."}]}; include one result per criterion, a concrete observation, actual 0–255 check exit status, and nonempty artifact beneath /evidence; report actual HEAD even if a criterion fails.`,
              requiredRole: "Verifier",
              finalVerification: true,
              acceptanceCriteria: plan.acceptanceCriteria,
            });
            for (const accepted of acceptedSlices)
              core.addDependency(
                context(core, credential),
                finalWorkItemId,
                accepted.workItemId,
              );
            blocker = await evaluateSupervisor();
            if (!blocker) {
              core.markReady(context(core, credential), finalWorkItemId);
              const finalAssignment = core.assignWorkItem(
                context(core, credential),
                finalWorkItemId,
                identities.Verifier.seatId,
              );
              finalRuntime = await provisionRuntime(
                "Verifier",
                identities.Verifier.seatId,
                finalWorkItemId,
                finalAssignment.generation,
                finalAssignment.assignmentId,
                acceptedSlices.map(({ workspace, commitSha }) => ({
                  workspace,
                  commitSha,
                })),
              );
              finalVerificationSource = {
                workspace: finalRuntime.session.workspace,
                commitSha: finalRuntime.baseSha,
              };
              runtimeCommands[finalRuntime.session.sessionId] =
                finalAssignment.commandId;
              if (Date.now() >= deadlineMs)
                blocker = "maxRunMs exceeded before final-parent dispatch";
              else if (stopping) blocker = "run canceled by signal";
              else {
                await waitForDispatchPermission();
                dispatches += 1;
                core.transitionRuntimeSession(
                  context(core, credential),
                  finalRuntime.session.sessionId,
                  "working",
                );
                await finalRuntime.adapter.dispatchAndStart(
                  context(core, credential),
                  finalAssignment.commandId,
                );
                const finalReport = await waitForReport(
                  finalWorkItemId,
                  finalAssignment.assignmentId,
                );
                if (!finalReport)
                  blocker =
                    "final-parent Verifier report did not complete before the bounded run deadline";
                else if (
                  finalReport.assignmentId !== finalAssignment.assignmentId ||
                  finalReport.role !== "Verifier" ||
                  finalReport.inputRevision !== core.inputRevision
                )
                  throw new Error(
                    "final-parent Verifier report does not match its assignment",
                  );
                else {
                  await containRuntime(
                    finalRuntime.session,
                    finalAssignment.assignmentId,
                  );
                  const reply = objectRecord(
                    parseJsonWithoutDuplicateMembers(finalReport.reply),
                  );
                  const finalSafeGit = [
                    "--no-replace-objects",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    `--git-dir=${path.join(finalRuntime.session.workspace, ".git")}`,
                    `--work-tree=${finalRuntime.session.workspace}`,
                  ];
                  const composedHead = spawnSync(
                    "git",
                    [
                      ...finalSafeGit,
                      "-C",
                      finalRuntime.session.workspace,
                      "rev-parse",
                      "HEAD",
                    ],
                    { encoding: "utf8", timeout: 10_000, env: verifiedGitEnv },
                  );
                  if (
                    reply?.commitSha !== finalRuntime.baseSha ||
                    reply.startingSha !== finalRuntime.baseSha ||
                    composedHead.status !== 0 ||
                    composedHead.stdout.trim().toLowerCase() !==
                      finalRuntime.baseSha.toLowerCase() ||
                    !Array.isArray(reply.evidence) ||
                    reply.evidence.length !== plan.acceptanceCriteria.length
                  )
                    throw new Error(
                      "final-parent Verifier did not preserve and report the exact composed HEAD or parent criteria",
                    );
                  assertTrackedCheckoutMatchesHead(
                    finalRuntime.session.workspace,
                    finalRuntime.baseSha,
                  );
                  const seen = new Set<string>();
                  const finalEvidence: {
                    evidenceId: string;
                    criterion: string;
                    passed: boolean;
                    observation: string;
                    exitStatus: number;
                    artifactRef: string;
                  }[] = [];
                  for (const rawEvidence of reply.evidence) {
                    const entry = objectRecord(rawEvidence);
                    if (
                      typeof entry?.criterion !== "string" ||
                      !plan.acceptanceCriteria.includes(entry.criterion) ||
                      seen.has(entry.criterion) ||
                      typeof entry.passed !== "boolean" ||
                      typeof entry.observation !== "string" ||
                      !entry.observation.trim() ||
                      !Number.isInteger(entry.exitStatus) ||
                      (entry.exitStatus as number) < 0 ||
                      (entry.exitStatus as number) > 255 ||
                      (entry.passed === true && entry.exitStatus !== 0) ||
                      typeof entry.artifactRef !== "string" ||
                      !entry.artifactRef.startsWith("/evidence/")
                    )
                      throw new Error(
                        "final-parent Verifier evidence is not the exact parent criterion set",
                      );
                    seen.add(entry.criterion);
                    finalEvidence.push({
                      evidenceId: randomUUID(),
                      criterion: entry.criterion,
                      passed: entry.passed,
                      observation: entry.observation,
                      exitStatus: entry.exitStatus as number,
                      artifactRef: verifiedEvidencePath(
                        finalRuntime.evidenceDirectory,
                        entry.artifactRef,
                      ),
                    });
                  }
                  const rejected = finalEvidence.find((entry) => !entry.passed);
                  if (rejected)
                    blocker = `final-parent Verifier rejected composed checkout for criterion ${rejected.criterion}`;
                  else {
                    blocker = await evaluateSupervisor();
                    if (!blocker)
                      core.acceptFinalVerification(context(core, credential), {
                        workItemId: finalWorkItemId,
                        assignmentId: finalAssignment.assignmentId,
                        commitSha: finalRuntime.baseSha,
                        evidence: finalEvidence,
                      });
                  }
                }
              }
            }
            if (!stopping && !blocker && finalRuntime)
              blocker = await evaluateSupervisor();
          }
        }
        if (stopping) blocker = "run canceled by signal";
        else if (!blocker && Date.now() >= deadlineMs)
          blocker = "maxRunMs exceeded";
        else if (!blocker && step.state === "stopped") blocker = step.reason;
        const cleanupErrors: unknown[] = [];
        const closeServer = closeControl;
        closeControl = undefined;
        try {
          await closeServer?.();
        } catch (error) {
          cleanupErrors.push(error);
        }
        await Promise.all([...cancellations]);
        await containPendingSessions(cleanupErrors);
        for (const adapter of Object.values(adapters)) {
          try {
            await adapter.close();
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        if (cleanupErrors.length)
          throw new AggregateError(
            cleanupErrors,
            `cstan cleanup could not prove complete containment and closure: ${cleanupErrors.map(String).join("; ")}`,
          );
        await manager.close();
        runtimeManagerClosed = true;
        if (stopping) blocker = "run canceled by signal";
        else if (!blocker && Date.now() >= deadlineMs)
          blocker = "maxRunMs exceeded";
        const completed = step.state === "complete" && !stopping && !blocker;
        if (completed)
          core.transitionRun(context(core, credential), "completed");
        else if (stopping) {
          const runState = core.statusSnapshot().run.state;
          if (runState !== "canceled") {
            if (runState !== "canceling")
              core.transitionRun(context(core, credential), "canceling");
            if (!provisionFailureUnproven)
              core.transitionRun(context(core, credential), "canceled");
          }
        } else if (supervisorMustPause)
          core.transitionRun(context(core, credential), "paused");
        else core.transitionRun(context(core, credential), "failed");
        const canceled =
          stopping && core.statusSnapshot().run.state === "canceled";
        if (stopping && !canceled)
          blocker =
            "runtime provisioning failed without a containment proof; run remains canceling";
        const runOutput: CstanRunJsonV1 = {
          schemaVersion: 1,
          state: completed
            ? "complete"
            : canceled
              ? "canceled"
              : stopping || step.state === "stopped"
                ? "stopped"
                : "waiting",
          projectId: core.projectId,
          planHash: validateWorkflowPlan(plan).hash,
          baseSha,
          roles: roleNames.map((role) => ({
            role,
            sessionId: sessions[identities[role].seatId]?.sessionId ?? null,
          })),
          scheduler: step,
          ...(blocker ? { blocker } : {}),
        };
        output(runOutput, true);
        cleanupComplete = true;
        return completed
          ? EXIT.ok
          : stopping && !canceled
            ? EXIT.runtime
            : EXIT.blocked;
      } finally {
        const cleanupErrors: unknown[] = [];
        if (!cleanupComplete) {
          const closeServer = closeControl;
          closeControl = undefined;
          try {
            await closeServer?.();
          } catch (error) {
            cleanupErrors.push(error);
          }
          await Promise.all([...cancellations]);
          await containPendingSessions(cleanupErrors);
          for (const adapter of Object.values(adapters)) {
            try {
              await adapter.close();
            } catch (error) {
              cleanupErrors.push(error);
            }
          }
          if (cleanupErrors.length)
            process.stderr.write(
              `cstan cleanup warning: ${cleanupErrors.map(String).join("; ")}\n`,
            );
        }
        if (cleanupErrors.length === 0) {
          try {
            const runState = core.statusSnapshot().run.state;
            if (stopping && runState !== "canceled") {
              if (runState !== "canceling")
                core.transitionRun(context(core, credential), "canceling");
              if (!operatorCancellationIncomplete && !provisionFailureUnproven)
                core.transitionRun(context(core, credential), "canceled");
            } else if (
              runState === "active" ||
              (runState === "paused" && Date.now() >= deadlineMs)
            )
              core.transitionRun(context(core, credential), "failed");
          } catch (error) {
            process.stderr.write(
              `cstan cleanup warning: could not mark aborted run terminal: ${String(error)}\n`,
            );
          }
        }
      }
    } finally {
      try {
        if (!runtimeManagerClosed) {
          await manager.close();
          runtimeManagerClosed = true;
        }
        if (
          runtimeSocketRoot &&
          runtimeManagerClosed &&
          allSessions.every((session) =>
            containedSessions.has(session.sessionId),
          )
        ) {
          fs.rmSync(runtimeSocketRoot, { recursive: true, force: true });
        }
      } finally {
        removeSignalHandlers();
        core.close();
      }
    }
  }
  if (command === "cancel") {
    const routed = parseOptions(rest);
    if (routed.positional.length === 1)
      return await runRouted("cancel", routed.positional, cwd, routed.json);
  }
  if (command === "pause" || command === "resume" || command === "cancel") {
    const parsed = parseOptions(rest);
    if (parsed.positional.length !== 0) usage();
    const { config, credential } = loadConfig(cwd);
    const socketPath = path.join(config.stateDirectory, "control.sock");
    let result: unknown;
    try {
      result = await requestControl(socketPath, credential, command);
    } catch (error) {
      if (isControllerUnreachable(error))
        throw new BlockedError(
          `${command} requires the foreground controller; run --brief is required for restart reconciliation`,
        );
      throw error;
    }
    output({ schemaVersion: 1, ...objectRecord(result) }, parsed.json);
    return EXIT.ok;
  }
  const beforeSeparator = rest.includes("--")
    ? rest.slice(0, rest.indexOf("--"))
    : rest;
  if (command === "status" && beforeSeparator.includes("--watch")) {
    const flags = [...rest];
    flags.splice(flags.indexOf("--watch"), 1);
    let intervalSeconds = 2;
    const at = flags.indexOf("--interval");
    if (at >= 0) {
      const value = flags[at + 1];
      intervalSeconds = Number(value);
      if (
        value === undefined ||
        !/^[1-9][0-9]?$/.test(value) ||
        intervalSeconds > 60
      )
        throw new InvalidInputError(
          "--interval must be an integer from 1 to 60",
        );
      flags.splice(at, 2);
    }
    if (flags.length !== 0) usage();
    const operator = await ensureRunning(cwd);
    await watchStatus({
      intervalMs: intervalSeconds * 1000,
      fetch: async () => {
        const result = await callDaemon(
          operator.socketPath,
          operator.credential,
          "status",
        );
        if (result.kind !== "response" || !result.response.ok)
          throw new Error("status is unavailable");
        return result.response.result as Record<string, unknown>;
      },
      write: (text) => void process.stdout.write(text),
      sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    });
    return EXIT.ok;
  }
  if (command === "status" && agentEnvironment()) {
    const parsed = parseOptions(rest);
    if (parsed.positional.length !== 0) usage();
    return await runRouted("status", [], cwd, parsed.json);
  }
  if (command === "status") {
    const { config, credential } = loadConfig(cwd);
    const parsed = parseOptions(rest);
    if (parsed.positional.length !== 0) usage();
    const snapshot = (await inspectController(
      config,
      credential,
      "status",
    )) as ControllerStatus;
    const blockers = snapshot.work
      .filter((item) => item.state !== "accepted")
      .map((item) => ({
        workItemId: item.workItemId,
        state: item.state,
        blockers: item.blockers,
      }));
    const nextLegalActions = [
      ...new Set(snapshot.work.flatMap((item) => item.nextLegalActions)),
    ];
    const statusOutput: CstanStatusJsonV1 = {
      schemaVersion: 1,
      ...snapshot,
      ownership: snapshot.work.map((item) => ({
        workItemId: item.workItemId,
        role: item.role,
        owner: item.owner,
      })),
      blockers,
      limits: {
        maxSlices: config.maxSlices,
        maxRunMs: config.maxRunMs,
        maxDispatches: config.maxDispatches,
      },
      nextLegalActions: nextLegalActions.length
        ? nextLegalActions
        : snapshot.run.state === "not_started"
          ? ["cstan run --brief <file>"]
          : [],
    };
    output(statusOutput, parsed.json);
    return EXIT.ok;
  }
  if (command === "inspect") {
    const parsed = parseOptions(rest);
    const id = parsed.positional[0];
    if (!id || parsed.positional.length !== 1) usage();
    const { config, credential } = loadConfig(cwd);
    const inspected = await inspectController(
      config,
      credential,
      "inspect",
      id,
    );
    if (parsed.json) {
      const value = objectRecord(inspected);
      const kinds = [
        "work_item",
        "assignment",
        "candidate",
        "finding",
        "recovery",
      ] as const;
      if (
        !value ||
        typeof value.kind !== "string" ||
        !kinds.includes(value.kind as CstanInspectJsonV1["kind"]) ||
        typeof value.id !== "string" ||
        !objectRecord(value.record)
      )
        throw new Error(
          "Controller returned an inspect record outside the cstan inspect JSON contract",
        );
      const inspectOutput: CstanInspectJsonV1 = {
        schemaVersion: 1,
        kind: value.kind as CstanInspectJsonV1["kind"],
        id: value.id,
        record: value.record as Record<string, unknown>,
      };
      output(inspectOutput, true);
    } else output(inspected, false);
    return EXIT.ok;
  }
  usage();
}

const isMain =
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  runCli(process.argv.slice(2))
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error: unknown) => {
      if (process.exitCode === undefined || process.exitCode === 0)
        process.exitCode =
          error instanceof BlockedError
            ? EXIT.blocked
            : error instanceof InvalidInputError ||
                error instanceof TypeError ||
                error instanceof SyntaxError
              ? EXIT.invalid
              : EXIT.runtime;
      process.stderr.write(
        `cstan: ${error instanceof Error ? error.message : String(error)}\n`,
      );
    });
}

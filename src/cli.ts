#!/usr/bin/env node
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  ControllerCore,
  type ControllerStatus,
  StateVersionConflictError,
} from "./controller/core.js";
import type {
  InitialProject,
  MutationContext,
  ProjectInput,
  RoleSyncResult,
} from "./controller/types.js";
import { requestControl } from "./control.js";
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
import {
  ControllerOwnershipError,
  ProjectLockHeldError,
} from "./controller/ownership.js";
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
  kind:
    | "work_item"
    | "assignment"
    | "candidate"
    | "finding"
    | "recovery"
    | "report";
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
      ![
        "status",
        "ping",
        "shutdown",
        "cancel",
        "pm-restart",
        "launch",
      ].includes(name),
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
    return error.reason === "refused"
      ? new BlockedError(error.message)
      : new Error(error.message);
  return error instanceof Error ? error : new Error(String(error));
}

const LAUNCHER_CLIENT_TIMEOUT_MS = 600_000;

/** CAPSTAN_LAUNCH=off keeps the daemon away from Herdr: no driver, no launcher, no PM launch at start. The test suite sets it so no test touches a real Herdr session. */
function launchDisabled(): boolean {
  return ["off", "0", "false", "no"].includes(
    (process.env.CAPSTAN_LAUNCH ?? "").trim().toLowerCase(),
  );
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
        `message ${m.messageId ?? ""} [${m.state ?? ""}] from ${m.from ?? ""}${m.fromAgentId === m.from ? "" : ` (${m.fromAgentId ?? ""})`}\n${m.body ?? ""}`,
    )
    .join("\n\n");
}

function handleWire(result: WireResult, json: boolean, command = ""): number {
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
  if (args.some((value) => value.includes("\ufffd")))
    throw new InvalidInputError(
      "a command argument holds a replacement character, so its text was not valid UTF-8",
    );
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
        command === "wait"
          ? WAIT_CLIENT_TIMEOUT_MS
          : command === "replace"
            ? 3 * LAUNCHER_CLIENT_TIMEOUT_MS
            : command === "spawn" ||
                command === "request-review" ||
                command === "integrate" ||
                command === "release" ||
                command === "pm-restart"
              ? LAUNCHER_CLIENT_TIMEOUT_MS
              : undefined,
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
    "usage: cstan init | cstan start | cstan stop | cstan ping | cstan config check | cstan config sync | cstan status [--json] | cstan status --watch [--interval <seconds>] | cstan inspect <id> [--json] | cstan cancel <id> [--json] | cstan inbox | cstan ack | cstan wait | cstan report | cstan ask | cstan finding <agent-id> <severity> <evidence> <correction> <done-when> | cstan finding check <finding-id> resolved|unresolved <evidence> | cstan observe <agent-id> [lines] | cstan assign | cstan send | cstan resolve | cstan spawn <role> | cstan release <agent-id> | cstan replace <agent-id> | cstan request-review <report-or-integration-id> [role] | cstan integrate <report-id>... | cstan integrate confirm|discard <integration-id> | cstan review pass|findings <text> | cstan pm restart",
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
      for (const role of roleConfig.roles) {
        const host = roleConfig.hosts.find((h) => h.name === role.host);
        if (host !== undefined && host.kind !== "claude")
          process.stderr.write(
            `warning: role ${role.name} runs on ${host.kind} with full access and no approval prompts; nothing blocks it from editing outside its worktree or from pushing\n`,
          );
      }
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
    let core: ControllerCore;
    try {
      core = await ControllerCore.open({
        stateDirectory: config.stateDirectory,
        project: project(config, credential, []),
        workspaceRoot: cwd,
      });
    } catch (error) {
      if (error instanceof ProjectLockHeldError)
        throw new BlockedError(
          `${error.message}; stop the daemon with cstan stop, then run cstan config sync again`,
        );
      throw error;
    }
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
      capstan === undefined || launchDisabled()
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
        cliPath: fileURLToPath(import.meta.url),
        ...(capstan === undefined
          ? {}
          : {
              syncRoles: (core: ControllerCore) => {
                syncConfiguredRoles(core, capstan, () =>
                  context(core, credential),
                );
              },
            }),
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
      let launch: unknown;
      let failed = false;
      if (
        fs.existsSync(path.join(cwd, CONFIG_FILE_NAME)) &&
        !launchDisabled()
      ) {
        // The daemon is up whatever happens here, so a launch that cannot be
        // reported is shown as the launch's own failure, not as a missing controller.
        try {
          const launched = await callDaemon(
            operator.socketPath,
            operator.credential,
            "launch",
            [],
            LAUNCHER_CLIENT_TIMEOUT_MS,
          );
          if (launched.kind !== "response") {
            launch = "launch: the controller answered in an unexpected form";
            failed = true;
          } else if (launched.response.ok) {
            launch = launched.response.result;
            failed =
              typeof launch === "object" &&
              launch !== null &&
              (launch as { state?: string }).state === "failed";
          } else {
            launch = `${launched.response.code}: ${launched.response.message}${
              launched.response.code === "not_configured"
                ? " (a daemon started before capstan.toml existed reads it only at start: run cstan stop and cstan start)"
                : ""
            }`;
            failed = true;
          }
        } catch (error) {
          launch = `launch: ${error instanceof Error ? error.message : String(error)}`;
          failed = true;
        }
      }
      output(
        {
          running: true,
          pid: result.pid,
          started: result.started,
          ...(launch === undefined ? {} : { launch }),
        },
        parsed.json,
      );
      if (failed) return EXIT.blocked;
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
  if (command === "cancel") {
    const routed = parseOptions(rest);
    if (routed.positional.length === 1)
      return await runRouted("cancel", routed.positional, cwd, routed.json);
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
        if (!result.response.ok) throw new Error("status is unavailable");
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
      nextLegalActions,
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
        "report",
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

// argv[1] is a symlink when run through `npm link` or a global install, so
// both sides are compared by their real paths.
function realPathOrSelf(file: string): string {
  // `node dist/src/cli` runs cli.js without its extension, so a path that is
  // not a file is retried with .js.
  for (const candidate of [file, `${file}.js`])
    try {
      const real = fs.realpathSync(candidate);
      if (fs.statSync(real).isFile()) return real;
    } catch {
      // Try the next spelling.
    }
  return path.resolve(file);
}

const isMain =
  process.argv[1] !== undefined &&
  process.argv[1] !== "" &&
  realPathOrSelf(process.argv[1]) ===
    realPathOrSelf(fileURLToPath(import.meta.url));
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

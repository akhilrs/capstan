/**
 * The Capstan controller daemon: one long-running process that owns the
 * project lock and serves the cstan command surface over a Unix socket.
 *
 * Known limit (DEC-005, MVP_PLAN_V2 section 6): any process of the same user
 * can reach the socket and can read an agent's environment token or the
 * operator key file. Authentication labels a caller; the controller's fact
 * verification narrows what a forger can do but does not close it.
 *
 * Credentials are never logged, echoed, stored or put in an error message.
 */
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { AuthenticationError } from "./controller/auth.js";
import { ControllerCore } from "./controller/core.js";
import type { Identity, InitialProject } from "./controller/types.js";

export const MAX_FRAME_BYTES = 65_536;
export const MAX_RESPONSE_BYTES = 1_048_576;
const MAX_ARGS = 16;
const REQUEST_TIMEOUT_MS = 5_000;
const MAX_CONNECTIONS = 64;
export const SOCKET_NAME = "control.sock";
export const PID_NAME = "daemon.pid";

export type CommandAccess = "read" | "agent" | "operator";
export type ErrorCode =
  | "unauthorized"
  | "forbidden"
  | "unknown_command"
  | "invalid_request"
  | "not_implemented"
  | "conflict"
  | "error";

export type CommandResponse =
  | { readonly ok: true; readonly result: unknown }
  | { readonly ok: false; readonly code: ErrorCode; readonly message: string };

interface Route {
  readonly access: CommandAccess;
  readonly stub?: string;
}

const STUB_STAGE = "Stage 2e";

export const ROUTES: Readonly<Record<string, Route>> = {
  status: { access: "read" },
  ping: { access: "read" },
  inbox: { access: "agent", stub: STUB_STAGE },
  ack: { access: "agent", stub: STUB_STAGE },
  wait: { access: "agent", stub: STUB_STAGE },
  report: { access: "agent", stub: STUB_STAGE },
  ask: { access: "agent", stub: STUB_STAGE },
  "request-review": { access: "agent", stub: STUB_STAGE },
  finding: { access: "agent", stub: STUB_STAGE },
  assign: { access: "operator", stub: STUB_STAGE },
  cancel: { access: "operator", stub: STUB_STAGE },
  send: { access: "operator", stub: STUB_STAGE },
  "pm-restart": { access: "operator", stub: STUB_STAGE },
  resolve: { access: "operator", stub: STUB_STAGE },
  shutdown: { access: "operator" },
};

export interface VerificationHooks {
  generationIsCurrent(agentId: string, generation: number): boolean;
  commitOnAgentBranch(agentId: string, sha: string): boolean | undefined;
  transitionLegal(
    entity: string,
    from: string,
    to: string,
  ): boolean | undefined;
}

export function defaultVerificationHooks(
  core: ControllerCore,
): VerificationHooks {
  return {
    generationIsCurrent: (agentId, generation) => {
      const agent = core.agentRecord(agentId);
      return agent?.state === "active" && agent.generation === generation;
    },
    commitOnAgentBranch: () => undefined,
    transitionLegal: () => undefined,
  };
}

export interface LogEntry {
  readonly command: string;
  readonly actorId: string | null;
  readonly role: string | null;
  readonly code: string;
  readonly ms: number;
  readonly argCount?: number;
  readonly argBytes?: number;
}

export type Logger = (entry: LogEntry) => void;

export interface DaemonServer {
  readonly closed: Promise<void>;
  close(): Promise<void>;
}

const LEGACY_FOREGROUND_ONLY =
  "pause, resume and cancel require the foreground cstan run controller";

let responseLimit = MAX_RESPONSE_BYTES;

function respond(
  socket: net.Socket,
  response: CommandResponse | Record<string, unknown>,
): void {
  let text = JSON.stringify(response);
  if (Buffer.byteLength(text) + 1 > responseLimit)
    text = JSON.stringify(failure("error", "response exceeds the size limit"));
  socket.end(`${text}\n`);
}

function failure(code: ErrorCode, message: string): CommandResponse {
  return { ok: false, code, message };
}

type Actor = "operator" | "agent" | "other";

function classify(identity: Identity): Actor {
  if (identity.role === "operator") return "operator";
  return identity.agent !== null ? "agent" : "other";
}

function allowed(
  access: CommandAccess,
  actor: Exclude<Actor, "other">,
): boolean {
  if (access === "read") return true;
  return access === actor;
}

function parseFrame(frame: Buffer): unknown {
  return JSON.parse(
    new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(frame),
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export async function startDaemonServer(options: {
  socketPath: string;
  core: ControllerCore;
  log: Logger;
  onShutdown: () => void;
  maxResponseBytes?: number;
}): Promise<DaemonServer> {
  const { socketPath, core, log, onShutdown } = options;
  removeStaleSocket(socketPath);
  responseLimit = options.maxResponseBytes ?? MAX_RESPONSE_BYTES;

  const handle = async (frame: Buffer, socket: net.Socket): Promise<void> => {
    const started = Date.now();
    let request: unknown;
    try {
      request = parseFrame(frame);
    } catch {
      respond(socket, failure("invalid_request", "request is not valid JSON"));
      log({
        command: "?",
        actorId: null,
        role: null,
        code: "invalid_request",
        ms: Date.now() - started,
      });
      return;
    }
    if (isRecord(request) && "action" in request && !("command" in request)) {
      handleLegacy(request, socket, started);
      return;
    }
    if (
      !isRecord(request) ||
      request.v !== 1 ||
      typeof request.command !== "string" ||
      request.command.length === 0 ||
      request.command.length > 64
    ) {
      respond(socket, failure("invalid_request", "malformed command request"));
      log({
        command: "?",
        actorId: null,
        role: null,
        code: "invalid_request",
        ms: Date.now() - started,
      });
      return;
    }
    const command = request.command;
    let identity: Identity;
    try {
      if (typeof request.credential !== "string")
        throw new AuthenticationError("invalid credential");
      identity = core.identify(request.credential);
    } catch (error) {
      if (!(error instanceof AuthenticationError)) throw error;
      respond(socket, failure("unauthorized", "credential not accepted"));
      log({
        command,
        actorId: null,
        role: null,
        code: "unauthorized",
        ms: Date.now() - started,
      });
      return;
    }
    const finish = (
      response: CommandResponse,
      extra: Partial<LogEntry> = {},
    ): void => {
      respond(socket, response);
      log({
        command,
        actorId: identity.actorId,
        role: identity.role,
        code: response.ok ? "ok" : response.code,
        ms: Date.now() - started,
        ...extra,
      });
    };
    const actor = classify(identity);
    if (actor === "other")
      return finish(
        failure("forbidden", "the caller is not an operator or an agent"),
      );
    const route = Object.hasOwn(ROUTES, command) ? ROUTES[command] : undefined;
    if (route === undefined)
      return finish(failure("unknown_command", "unknown command"));
    if (!allowed(route.access, actor))
      return finish(
        failure(
          "forbidden",
          route.access === "operator"
            ? "this command needs the operator credential"
            : "this command needs an agent token",
        ),
      );
    const args = request.args ?? [];
    if (
      !Array.isArray(args) ||
      args.length > MAX_ARGS ||
      args.some((value) => typeof value !== "string")
    )
      return finish(
        failure("invalid_request", `args must be at most ${MAX_ARGS} strings`),
      );
    const argBytes = (args as string[]).reduce(
      (total, value) => total + Buffer.byteLength(value),
      0,
    );
    const extra = { argCount: args.length, argBytes };
    if (route.stub !== undefined)
      return finish(
        failure(
          "not_implemented",
          `${command} is not implemented yet (${route.stub})`,
        ),
        extra,
      );
    switch (command) {
      case "ping":
        return finish({ ok: true, result: { pong: true, pid: process.pid } });
      case "status":
        return finish({
          ok: true,
          result: {
            ...(core.statusSnapshot() as unknown as Record<string, unknown>),
            agents: core.listAgents(),
          },
        });
      case "shutdown":
        finish({ ok: true, result: { stopping: true } });
        setImmediate(onShutdown);
        return;
      default:
        return finish(failure("error", "command has no handler"));
    }
  };

  const handleLegacy = (
    request: Record<string, unknown>,
    socket: net.Socket,
    started: number,
  ): void => {
    let identity: Identity | undefined;
    try {
      if (typeof request.token === "string")
        identity = core.identify(request.token);
    } catch (error) {
      if (!(error instanceof AuthenticationError)) throw error;
    }
    const action =
      typeof request.action === "string" ? request.action.slice(0, 32) : "?";
    const done = (body: Record<string, unknown>, code: string): void => {
      respond(socket, body);
      log({
        command: `legacy:${action}`,
        actorId: identity?.actorId ?? null,
        role: identity?.role ?? null,
        code,
        ms: Date.now() - started,
      });
    };
    if (identity === undefined || identity.role !== "operator")
      return done({ error: "unauthorized" }, "unauthorized");
    if (action === "status" && request.id === undefined)
      return done(
        { requestId: randomUUID(), result: core.statusSnapshot() },
        "ok",
      );
    if (
      action === "inspect" &&
      typeof request.id === "string" &&
      request.id.length > 0
    ) {
      try {
        return done(
          { requestId: randomUUID(), result: core.inspect(request.id) },
          "ok",
        );
      } catch (error) {
        return done(
          { error: error instanceof Error ? error.message : "inspect failed" },
          "error",
        );
      }
    }
    if (["pause", "resume", "cancel"].includes(action))
      return done({ error: LEGACY_FOREGROUND_ONLY }, "not_supported");
    return done({ error: "invalid control request" }, "invalid_request");
  };

  const server = net.createServer((socket) => {
    socket.on("error", () => socket.destroy());
    socket.setTimeout(REQUEST_TIMEOUT_MS, () => socket.destroy());
    const chunks: Buffer[] = [];
    let received = 0;
    socket.on("data", (chunk: Buffer) => {
      const newline = chunk.indexOf(10);
      if (newline < 0) {
        chunks.push(chunk);
        received += chunk.length;
        if (received > MAX_FRAME_BYTES) socket.destroy();
        return;
      }
      if (received + newline > MAX_FRAME_BYTES) {
        socket.destroy();
        return;
      }
      const frame = Buffer.concat([...chunks, chunk.subarray(0, newline)]);
      socket.removeAllListeners("data");
      handle(frame, socket).catch(() => {
        respond(socket, failure("error", "internal error"));
        log({
          command: "?",
          actorId: null,
          role: null,
          code: "error",
          ms: 0,
        });
      });
    });
  });
  server.maxConnections = MAX_CONNECTIONS;
  server.on("error", () =>
    log({
      command: "server",
      actorId: null,
      role: null,
      code: "server_error",
      ms: 0,
    }),
  );

  const previousUmask = process.umask(0o077);
  try {
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(socketPath, () => {
        process.umask(previousUmask);
        resolve();
      });
    });
  } finally {
    process.umask(previousUmask);
  }
  let identity: fs.Stats;
  try {
    fs.chmodSync(socketPath, 0o600);
    identity = fs.lstatSync(socketPath);
    if (!identity.isSocket()) throw new Error("control socket was not created");
  } catch (error) {
    server.close();
    try {
      if (fs.lstatSync(socketPath).isSocket()) fs.unlinkSync(socketPath);
    } catch {
      // nothing left to clean up
    }
    throw error;
  }

  let closedResolve!: () => void;
  const closed = new Promise<void>((resolve) => {
    closedResolve = resolve;
  });
  return {
    closed,
    close: async () => {
      if (server.listening)
        await new Promise<void>((resolve, reject) =>
          server.close((error) => (error ? reject(error) : resolve())),
        );
      try {
        const current = fs.lstatSync(socketPath);
        if (
          current.isSocket() &&
          current.dev === identity.dev &&
          current.ino === identity.ino
        )
          fs.unlinkSync(socketPath);
      } catch (error) {
        if (!(
          error instanceof Error &&
          "code" in error &&
          error.code === "ENOENT"
        ))
          throw error;
      }
      closedResolve();
    },
  };
}

/**
 * The caller holds the project lock, and only the lock holder binds the
 * socket, so anything already at the path is stale by construction.
 */
export function removeStaleSocket(socketPath: string): void {
  if (!path.isAbsolute(socketPath))
    throw new TypeError("control socket path must be absolute");
  const directory = fs.lstatSync(path.dirname(socketPath));
  if (
    !directory.isDirectory() ||
    directory.isSymbolicLink() ||
    (process.getuid && directory.uid !== process.getuid()) ||
    (directory.mode & 0o077) !== 0
  )
    throw new Error(
      "control socket directory must be private and owned by the current user",
    );
  let stat: fs.Stats;
  try {
    stat = fs.lstatSync(socketPath);
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT")
      return;
    throw error;
  }
  if (!stat.isSocket())
    throw new Error("control socket path exists and is not a socket");
  if (process.getuid && stat.uid !== process.getuid())
    throw new Error("control socket path is owned by another user");
  fs.unlinkSync(socketPath);
}

// A fresh v2 project has no brief, criteria or plan yet, but the ledger needs
// one revision of each durable input kind to exist. The PM records the real
// inputs as later revisions; these placeholders are marked so nothing can
// mistake them for real inputs.
export const PLACEHOLDER_INPUTS: InitialProject["initialInputs"] = [
  { kind: "project_config", content: { placeholder: true } },
  { kind: "task_brief", content: { placeholder: true } },
  {
    kind: "acceptance_criteria",
    content: ["(no acceptance criteria recorded yet)"],
  },
  { kind: "policy", content: { placeholder: true } },
  { kind: "plan", content: { placeholder: true } },
];

export interface DaemonOptions {
  readonly stateDirectory: string;
  readonly project: InitialProject;
  readonly workspaceRoot: string;
  readonly log: Logger;
  readonly announce?: (event: { event: string; pid: number }) => void;
}

/** Runs until SIGTERM, SIGINT or the shutdown command. */
export async function runDaemon(options: DaemonOptions): Promise<void> {
  const socketPath = path.join(options.stateDirectory, SOCKET_NAME);
  const pidPath = path.join(options.stateDirectory, PID_NAME);
  let stopRequested!: () => void;
  const stop = new Promise<void>((resolve) => {
    stopRequested = resolve;
  });
  // Handlers go in before open: a signal during migrations or reconcile must
  // still end in a clean close instead of killing the process mid-way.
  const signals: NodeJS.Signals[] = ["SIGTERM", "SIGINT"];
  const handler = (): void => stopRequested();
  for (const signal of signals) process.on(signal, handler);
  let core: ControllerCore | undefined;
  let server: DaemonServer | undefined;
  let stopping = false;
  void stop.then(() => {
    stopping = true;
  });
  try {
    core = await ControllerCore.open({
      stateDirectory: options.stateDirectory,
      project: options.project,
      workspaceRoot: options.workspaceRoot,
    });
    if (stopping) return;
    server = await startDaemonServer({
      socketPath,
      core,
      log: options.log,
      onShutdown: stopRequested,
    });
    writePidFile(pidPath);
    options.announce?.({ event: "ready", pid: process.pid });
    await stop;
  } finally {
    // The handlers stay until the very end so a second signal during close
    // cannot kill the process and leave the socket and pid file behind.
    if (server !== undefined) await server.close();
    if (core !== undefined) {
      removePidFile(pidPath);
      core.close();
    }
    for (const signal of signals) process.off(signal, handler);
  }
}

function writePidFile(pidPath: string): void {
  try {
    fs.unlinkSync(pidPath);
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "ENOENT"))
      throw error;
  }
  fs.writeFileSync(pidPath, `${process.pid}\n`, { flag: "wx", mode: 0o600 });
}

function removePidFile(pidPath: string): void {
  try {
    if (fs.readFileSync(pidPath, "utf8").trim() === String(process.pid))
      fs.unlinkSync(pidPath);
  } catch {
    // The pid file is advisory; the project lock is authoritative.
  }
}

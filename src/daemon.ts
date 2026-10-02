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
import { createCommandHandlers, type CommandSet } from "./commands.js";
import {
  branchTip,
  commitExists,
  deleteBranchAt,
  headCommit,
  inspectCommit,
  isInHead,
  mergeIntoBranch,
} from "./git.js";
import { recoverIntegrations } from "./integration.js";
import type { IntegrationGit } from "./integration.js";
import { startReportRelay, type ReportRelay } from "./reports.js";
import { startSupervision, type SupervisionHandle } from "./supervision.js";
import { recoverReviews } from "./reviews.js";
import type { CapstanConfig } from "./config/capstan-config.js";
import { newContext } from "./context.js";
import { DeliveryDriver, type DriverAdapter } from "./driver.js";
import { Launcher, type LauncherAdapter } from "./launcher.js";
import type { Notifier } from "./notifier.js";
import type { Identity, InitialProject } from "./controller/types.js";

export const MAX_FRAME_BYTES = 65_536;
export const MAX_RESPONSE_BYTES = 1_048_576;
const MAX_ARGS = 16;
const REQUEST_TIMEOUT_MS = 5_000;
const MAX_CONNECTIONS = 64;
const DRAIN_FLUSH_MS = 1_000;
export const SOCKET_NAME = "control.sock";
export const PID_NAME = "daemon.pid";

export type CommandAccess = "read" | "agent" | "operator" | "any";
export type ErrorCode =
  | "unauthorized"
  | "forbidden"
  | "unknown_command"
  | "invalid_request"
  | "not_implemented"
  | "conflict"
  | "rejected"
  | "shutting_down"
  | "superseded"
  | "unknown_recipient"
  | "unknown_agent"
  | "ambiguous_recipient"
  | "recipient_not_allowed"
  | "recipient_not_deliverable"
  | "self_send"
  | "body_too_large"
  | "not_configured"
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
  inbox: { access: "any" },
  ack: { access: "agent" },
  wait: { access: "agent" },
  report: { access: "agent" },
  ask: { access: "agent", stub: STUB_STAGE },
  "request-review": { access: "agent" },
  integrate: { access: "any" },
  plan: { access: "any" },
  review: { access: "agent" },
  finding: { access: "agent" },
  observe: { access: "agent" },
  peek: { access: "operator" },
  assign: { access: "operator", stub: STUB_STAGE },
  cancel: { access: "operator" },
  send: { access: "any" },
  "pm-restart": { access: "operator" },
  launch: { access: "operator" },
  spawn: { access: "any" },
  release: { access: "any" },
  replace: { access: "any" },
  resolve: { access: "operator" },
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
  readonly detail?: Record<string, unknown>;
}

export type Logger = (entry: LogEntry) => void;

export interface DaemonServer {
  readonly closed: Promise<void>;
  /** Stops accepting, drains every handler and closes; the socket file stays until cleanup(). */
  stop(): Promise<void>;
  /** Step 1 of an ordered stop: no new connections. */
  stopAccepting(): void;
  /** Step 3 of an ordered stop: abort handlers, await them, destroy what is left. */
  drain(): Promise<void>;
  /** Removes the socket file if it is still ours. */
  cleanup(): void;
  close(): Promise<void>;
}

function respond(
  socket: net.Socket,
  response: CommandResponse | Record<string, unknown>,
  limit: number,
): void {
  let text = JSON.stringify(response);
  if (Buffer.byteLength(text) + 1 > limit)
    text = JSON.stringify(failure("error", "response exceeds the size limit"));
  if (socket.destroyed || !socket.writable) return;
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
  if (access === "read" || access === "any") return true;
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

interface Connection {
  readonly socket: net.Socket;
  readonly controller: AbortController;
  timer: NodeJS.Timeout | undefined;
  dispatched: boolean;
  isWait: boolean;
}

export async function startDaemonServer(options: {
  socketPath: string;
  core: ControllerCore;
  log: Logger;
  onShutdown: () => void;
  commands?: CommandSet;
  maxResponseBytes?: number;
}): Promise<DaemonServer> {
  const { socketPath, core, log, onShutdown, commands } = options;
  const connections = new Set<Connection>();
  const running = new Set<Promise<void>>();
  let draining = false;
  let listenClosed: Promise<void> = Promise.resolve();
  const arm = (connection: Connection, ms: number): void => {
    clearTimeout(connection.timer);
    connection.timer = setTimeout(() => connection.socket.destroy(), ms);
  };
  removeStaleSocket(socketPath);
  const limit = options.maxResponseBytes ?? MAX_RESPONSE_BYTES;
  const send = (
    socket: net.Socket,
    response: CommandResponse | Record<string, unknown>,
  ): void => respond(socket, response, limit);

  const handle = async (
    frame: Buffer,
    socket: net.Socket,
    connection: Connection,
  ): Promise<void> => {
    const started = Date.now();
    let request: unknown;
    try {
      request = parseFrame(frame);
    } catch {
      send(socket, failure("invalid_request", "request is not valid JSON"));
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
      send(socket, failure("invalid_request", "malformed command request"));
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
      send(socket, failure("unauthorized", "credential not accepted"));
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
      send(socket, response);
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
      args.some((value) => typeof value !== "string" || value.length === 0)
    )
      return finish(
        failure(
          "invalid_request",
          `args must be at most ${MAX_ARGS} non-empty strings`,
        ),
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
    const handler = commands?.handlers[command];
    if (handler !== undefined) {
      const limitMs = commands?.limitMs(command, identity);
      if (limitMs !== undefined) {
        connection.isWait = true;
        arm(connection, Math.max(6_000, limitMs + REQUEST_TIMEOUT_MS));
      }
      const response = await handler({
        credential: request.credential as string,
        identity,
        args: args as string[],
        signal: connection.controller.signal,
        ...(limitMs === undefined ? {} : { limitMs }),
      });
      if (response === null) {
        log({
          command,
          actorId: identity.actorId,
          role: identity.role,
          code: "closed",
          ms: Date.now() - started,
          ...extra,
        });
        return;
      }
      return finish(response, extra);
    }
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
      send(socket, body);
      log({
        command: `control:${action}`,
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
    return done({ error: "invalid control request" }, "invalid_request");
  };

  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    const connection: Connection = {
      socket,
      controller: new AbortController(),
      timer: undefined,
      dispatched: false,
      isWait: false,
    };
    connections.add(connection);
    socket.on("error", () => socket.destroy());
    socket.on("close", () => {
      clearTimeout(connection.timer);
      connections.delete(connection);
      if (!connection.controller.signal.aborted)
        connection.controller.abort("closed");
    });
    socket.on("end", () => {
      // A client that half-closes after its frame still gets its reply, except
      // for wait, which the client cannot outlive; before a frame it is a drop.
      if (!connection.dispatched || connection.isWait) {
        if (!connection.controller.signal.aborted)
          connection.controller.abort("closed");
        socket.destroy();
      }
    });
    if (draining) {
      send(socket, failure("shutting_down", "the daemon is shutting down"));
      return;
    }
    arm(connection, REQUEST_TIMEOUT_MS);
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
      // Later bytes are discarded but the socket keeps flowing, so a closed
      // client is noticed at once and extra bytes cannot extend the deadline.
      socket.on("data", () => undefined);
      connection.dispatched = true;
      const work: Promise<void> = handle(frame, socket, connection)
        .catch(() => {
          send(socket, failure("error", "internal error"));
          log({
            command: "?",
            actorId: null,
            role: null,
            code: "error",
            ms: 0,
          });
        })
        .finally(() => running.delete(work));
      running.add(work);
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
  const stopAccepting = (): void => {
    draining = true;
    if (server.listening)
      listenClosed = new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
  };
  const drain = async (): Promise<void> => {
    for (const connection of connections)
      if (!connection.controller.signal.aborted)
        connection.controller.abort("shutdown");
    await Promise.allSettled([...running]);
    // A socket that was already answered is ended and must flush its reply
    // (a shutting_down or superseded code) before it closes; only a socket
    // that never got a request is destroyed at once.
    for (const connection of connections)
      if (!connection.socket.writableEnded) connection.socket.destroy();
    const flushing = [...connections].map(
      (connection) =>
        new Promise<void>((resolve) =>
          connection.socket.once("close", resolve),
        ),
    );
    await Promise.race([
      Promise.all(flushing),
      new Promise<void>((resolve) =>
        setTimeout(resolve, DRAIN_FLUSH_MS).unref(),
      ),
    ]);
    for (const connection of connections) connection.socket.destroy();
    await listenClosed;
  };
  const stop = async (): Promise<void> => {
    stopAccepting();
    await drain();
  };
  const cleanup = (): void => {
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
  };
  return {
    closed,
    stop,
    stopAccepting,
    drain,
    cleanup,
    close: async () => {
      await stop();
      cleanup();
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
  /** With an adapter and a notifier, the delivery driver runs; without them only the commands do. */
  readonly capstan?: CapstanConfig;
  readonly adapter?: DriverAdapter & LauncherAdapter;
  readonly notifier?: Notifier;
  readonly tickMs?: number;
  /** How often the supervision tick looks; a test makes it short. */
  readonly supervisionTickMs?: number;
  /** Absolute path of the CLI entry the launched agents' `cstan` wrapper runs. */
  readonly cliPath?: string;
  /** Brings the configured roles into the ledger as the daemon starts, so `cstan start` needs no separate `config sync`. */
  readonly syncRoles?: (core: ControllerCore) => void;
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
  let driver: DeliveryDriver | undefined;
  let launcher: Launcher | undefined;
  let adoption: Promise<void> = Promise.resolve();
  let reportRelay: ReportRelay | undefined;
  let supervision: SupervisionHandle | undefined;
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
    const credential = options.project.ownerCredential;
    const detailLog = (event: string, detail: Record<string, unknown>): void =>
      options.log({
        command: `daemon:${event}`,
        actorId: null,
        role: null,
        code: "info",
        ms: 0,
        detail,
      });
    closeStaleWaits(core, credential, detailLog);
    try {
      options.syncRoles?.(core);
    } catch (error) {
      // A role change the ledger refuses (for example a retired role that still
      // has an active agent) must not keep the daemon down; launch reports it.
      detailLog("role_sync_failed", { error: String(error) });
    }
    if (
      options.capstan !== undefined &&
      options.adapter !== undefined &&
      options.notifier !== undefined
    )
      driver = new DeliveryDriver({
        core,
        adapter: options.adapter,
        timers: options.capstan.timers,
        notifier: options.notifier,
        credential,
        log: detailLog,
        ...(options.tickMs === undefined ? {} : { tickMs: options.tickMs }),
      });
    if (
      options.capstan !== undefined &&
      options.adapter !== undefined &&
      options.cliPath !== undefined
    )
      launcher = new Launcher({
        core,
        adapter: options.adapter,
        config: options.capstan,
        projectRoot: options.workspaceRoot,
        cliPath: options.cliPath,
        socketPath,
        credential,
        log: detailLog,
        ...(options.syncRoles === undefined
          ? {}
          : { syncRoles: () => options.syncRoles!(core!) }),
      });
    const commands = createCommandHandlers({
      core,
      ...(options.capstan === undefined ? {} : { config: options.capstan }),
      ...(launcher === undefined ? {} : { launcher }),
      controllerCredential: credential,
      inspectCommit: (input) => inspectCommit(options.workspaceRoot, input),
      commitExists: (sha) => commitExists(options.workspaceRoot, sha),
      integrationGit: integrationGit(options.workspaceRoot),
      driverSnapshot: () =>
        driver?.snapshot() ?? { stalledAgentIds: [], stuck: [] },
      log: detailLog,
    });
    server = await startDaemonServer({
      socketPath,
      core,
      log: options.log,
      onShutdown: stopRequested,
      commands,
    });
    // Panes recorded before a restart are re-registered first, in the
    // background, so the socket binds at once; the driver starts only after.
    adoption =
      launcher === undefined
        ? Promise.resolve()
        : launcher.adoptAll().catch((error: unknown) => {
            detailLog("adopt_failed", { error: String(error) });
          });
    void adoption.then(() => {
      if (!stopping) driver?.start();
      if (!stopping && launcher !== undefined && options.capstan !== undefined)
        supervision = startSupervision({
          core: core!,
          launcher,
          credential,
          supervision: options.capstan.supervision,
          supervisorRole: options.capstan.roles.find(
            (role) => role.kind === "Supervisor",
          )?.name,
          intervalMs: options.supervisionTickMs ?? 15_000,
          log: detailLog,
        });
    });
    if (launcher !== undefined) {
      const reviewLauncher = launcher;
      void adoption
        .then(() =>
          stopping
            ? undefined
            : recoverReviews(
                { core: core!, launcher: reviewLauncher, log: detailLog },
                credential,
              ),
        )
        .catch((error) =>
          detailLog("review_recovery_failed", { error: String(error) }),
        );
    }
    void recoverIntegrations({
      core,
      git: integrationGit(options.workspaceRoot),
      context: (token) => newContext(core!, token),
      credential,
      log: detailLog,
    }).catch((error) =>
      detailLog("integration_recovery_failed", { error: String(error) }),
    );
    reportRelay = startReportRelay({
      core,
      credential,
      intervalMs: options.tickMs ?? 2000,
      log: detailLog,
      ...(options.capstan === undefined
        ? {}
        : {
            findingCheckSeconds: options.capstan.timers.findingCheckSeconds,
          }),
    });
    writePidFile(pidPath);
    options.announce?.({ event: "ready", pid: process.pid });
    await stop;
  } finally {
    // The handlers stay until the very end so a second signal during close
    // cannot kill the process and leave the socket and pid file behind. The
    // socket goes last: once it is gone, the project lock is already free, so
    // a stop followed by a start never loses the lock to a dying daemon.
    // Order: no new connections, no new ticks, then abort and await every
    // running handler, so nothing touches the database after it closes.
    server?.stopAccepting();
    reportRelay?.stop();
    await adoption;
    await supervision?.stop();
    await driver?.stop();
    if (server !== undefined) await server.drain();
    if (core !== undefined) {
      core.close();
      options.announce?.({ event: "lock_released", pid: process.pid });
    }
    if (server !== undefined) {
      server.cleanup();
      options.announce?.({ event: "socket_removed", pid: process.pid });
    }
    if (core !== undefined) removePidFile(pidPath);
    for (const signal of signals) process.off(signal, handler);
  }
}

function integrationGit(root: string): IntegrationGit {
  return {
    headCommit: () => headCommit(root),
    commitExists: (sha) => commitExists(root, sha),
    merge: (input) => mergeIntoBranch(root, input),
    branchTip: (branch) => branchTip(root, branch),
    isInHead: (sha) => isInHead(root, sha),
    deleteBranch: (branch, sha) => deleteBranchAt(root, branch, sha),
  };
}

/** A wait cannot outlive its connection, so any row still open at start belongs to a dead process. */
export function closeStaleWaits(
  core: ControllerCore,
  credential: string,
  log: (event: string, detail: Record<string, unknown>) => void,
): void {
  for (const wait of core.openWaits(credential))
    try {
      core.endWaitAsController(newContext(core, credential), wait.waitId);
    } catch (error) {
      log("stale_wait_not_closed", {
        waitId: wait.waitId,
        error: String(error),
      });
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

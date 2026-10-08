import { spawn } from "node:child_process";
import {
  ADDED_NODE_OPTION_VARIABLE,
  DAEMON_SEMI_SPACE_FLAG,
} from "./node-options.js";
import { isSea, selfCommand } from "./sea.js";

export {
  ADDED_NODE_OPTION_VARIABLE,
  DAEMON_SEMI_SPACE_FLAG,
  restoreNodeOptions,
} from "./node-options.js";
import fs from "node:fs";
import net from "node:net";
import {
  MAX_FRAME_BYTES,
  MAX_RESPONSE_BYTES,
  type CommandResponse,
} from "./daemon.js";

const DEFAULT_TIMEOUT_MS = 5_000;
// A cap, not a delay: a start returns the moment the daemon answers a ping, or
// fails the moment the child exits. A loaded machine can take well over 10 s.
const START_TIMEOUT_MS = 30_000;
const POLL_MS = 100;
// Long enough for the winning daemon to finish starting on a loaded machine.
const LOST_RACE_GRACE_MS = 20_000;

export type WireResult = {
  readonly kind: "response";
  readonly response: CommandResponse;
};

export class ControllerUnavailableError extends Error {
  override readonly name = "ControllerUnavailableError";
  constructor(
    readonly reason:
      "refused" | "unreachable" | "start_timeout" | "start_failed",
    message: string,
  ) {
    super(message);
  }
}

function withCode(message: string, code: string): NodeJS.ErrnoException {
  return Object.assign(new Error(message), { code });
}

export async function callDaemon(
  socketPath: string,
  credential: string,
  command: string,
  args: readonly string[] = [],
  timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<WireResult> {
  const frame = JSON.stringify({ v: 1, credential, command, args });
  if (Buffer.byteLength(frame) > MAX_FRAME_BYTES)
    throw new TypeError("command request is too large");
  return await new Promise<WireResult>((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let bytes = Buffer.alloc(0);
    let settled = false;
    const settle = (action: () => void): void => {
      if (settled) return;
      settled = true;
      socket.destroy();
      action();
    };
    socket.once("connect", () => socket.write(`${frame}\n`));
    socket.on("data", (chunk: Buffer) => {
      bytes = Buffer.concat([bytes, chunk]);
      const newline = bytes.indexOf(10);
      if (newline < 0) {
        if (bytes.length > MAX_RESPONSE_BYTES)
          settle(() => reject(withCode("response too large", "EMSGSIZE")));
        return;
      }
      try {
        const body: unknown = JSON.parse(
          new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
            bytes.subarray(0, newline),
          ),
        );
        if (typeof body !== "object" || body === null || Array.isArray(body))
          throw new Error("malformed reply");
        const record = body as Record<string, unknown>;
        if (!("ok" in record)) throw new Error("malformed reply");
        settle(() =>
          resolve({
            kind: "response",
            response: record as unknown as CommandResponse,
          }),
        );
      } catch {
        settle(() => reject(withCode("malformed reply", "EBADMSG")));
      }
    });
    socket.once("error", (error) => settle(() => reject(error)));
    socket.once("close", () =>
      settle(() => reject(withCode("connection closed", "ECONNRESET"))),
    );
    socket.setTimeout(timeoutMs, () =>
      settle(() => reject(withCode("timed out", "ETIMEDOUT"))),
    );
  });
}

export type PingOutcome =
  | { readonly outcome: "running"; readonly pid: number }
  | { readonly outcome: "refused"; readonly code: string }
  | { readonly outcome: "down" }
  | { readonly outcome: "unreachable"; readonly reason: string };

export async function pingDaemon(
  socketPath: string,
  credential: string,
  timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<PingOutcome> {
  try {
    const result = await callDaemon(
      socketPath,
      credential,
      "ping",
      [],
      timeoutMs,
    );
    if (result.response.ok) {
      const value = result.response.result as { pid?: unknown };
      return {
        outcome: "running",
        pid: typeof value.pid === "number" ? value.pid : 0,
      };
    }
    return { outcome: "refused", code: result.response.code };
  } catch (error) {
    const code =
      error instanceof Error && "code" in error ? String(error.code) : "";
    if (code === "ENOENT" || code === "ECONNREFUSED")
      return { outcome: "down" };
    return { outcome: "unreachable", reason: code || "unknown" };
  }
}

export function scrubEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const copy = { ...env };
  delete copy.CAPSTAN_TOKEN;
  delete copy.CAPSTAN_SOCKET;
  return copy;
}

/** The command, arguments and environment that start the daemon: the CLI's own, with the young-generation cap unless one is already set. */
export function daemonCommand(
  env: NodeJS.ProcessEnv,
  cliPath?: string,
  sea: boolean = isSea(),
): { command: string; args: string[]; env: NodeJS.ProcessEnv } {
  const self = selfCommand(["daemon"], cliPath);
  const capped = (list: readonly string[]): boolean =>
    list.some((flag) => flag.startsWith("--max-semi-space-size"));
  const environment = scrubEnvironment(env);
  delete environment[ADDED_NODE_OPTION_VARIABLE];
  if (!sea)
    return {
      ...self,
      args: capped(self.args)
        ? self.args
        : [DAEMON_SEMI_SPACE_FLAG, ...self.args],
      env: environment,
    };
  // The standalone binary has no node flags of its own; it reads them from NODE_OPTIONS.
  const options = environment.NODE_OPTIONS ?? "";
  if (!capped(options.split(/\s+/))) {
    environment.NODE_OPTIONS = `${options} ${DAEMON_SEMI_SPACE_FLAG}`.trim();
    environment[ADDED_NODE_OPTION_VARIABLE] = DAEMON_SEMI_SPACE_FLAG;
  }
  return { ...self, env: environment };
}

export function openDaemonLog(logPath: string): number {
  const flags =
    fs.constants.O_WRONLY | fs.constants.O_APPEND | fs.constants.O_NOFOLLOW;
  let fd: number;
  let created = false;
  try {
    fd = fs.openSync(
      logPath,
      flags | fs.constants.O_CREAT | fs.constants.O_EXCL,
      0o600,
    );
    created = true;
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "EEXIST"))
      throw error;
    fd = fs.openSync(logPath, flags);
  }
  try {
    // A file we just created gets its mode from the umask; an existing file
    // with a wrong mode is refused because someone else may have made it.
    if (created) fs.fchmodSync(fd, 0o600);
    const stat = fs.fstatSync(fd);
    if (
      !stat.isFile() ||
      (stat.mode & 0o777) !== 0o600 ||
      (process.getuid && stat.uid !== process.getuid())
    )
      throw new Error(
        "daemon log must be a regular file owned by the current user with mode 0600",
      );
    return fd;
  } catch (error) {
    fs.closeSync(fd);
    throw error;
  }
}

export interface EnsureOptions {
  readonly socketPath: string;
  readonly credential: string;
  readonly projectRoot: string;
  readonly logPath: string;
  readonly cliPath: string;
  readonly env: NodeJS.ProcessEnv;
  readonly timeoutMs?: number;
}

function unavailable(
  outcome: PingOutcome,
): ControllerUnavailableError | undefined {
  if (outcome.outcome === "refused")
    return new ControllerUnavailableError(
      "refused",
      `the daemon refused the request (${outcome.code}); check the operator credential`,
    );
  if (outcome.outcome === "unreachable")
    return new ControllerUnavailableError(
      "unreachable",
      "the controller did not answer; check the daemon log or retry",
    );
  return undefined;
}

export async function ensureDaemon(
  options: EnsureOptions,
): Promise<{ readonly pid: number; readonly started: boolean }> {
  const first = await pingDaemon(options.socketPath, options.credential);
  if (first.outcome === "running") return { pid: first.pid, started: false };
  const blocked = unavailable(first);
  if (blocked) throw blocked;

  const fd = openDaemonLog(options.logPath);
  const logOffset = fs.fstatSync(fd).size;
  let exitCode: number | undefined;
  let spawnError: string | undefined;
  let lostRaceAt: number | undefined;
  try {
    const self = daemonCommand(options.env, options.cliPath);
    const child = spawn(self.command, self.args, {
      cwd: options.projectRoot,
      env: self.env,
      detached: true,
      stdio: ["ignore", fd, fd],
    });
    child.unref();
    child.once("exit", (code) => {
      exitCode = code ?? 1;
    });
    child.once("error", (error) => {
      spawnError = error.message;
    });
  } finally {
    fs.closeSync(fd);
  }
  const deadline = Date.now() + (options.timeoutMs ?? START_TIMEOUT_MS);
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    const outcome = await pingDaemon(options.socketPath, options.credential);
    if (outcome.outcome === "running")
      return { pid: outcome.pid, started: true };
    if (spawnError !== undefined)
      throw new ControllerUnavailableError(
        "start_failed",
        `the daemon could not be started: ${spawnError}`,
      );
    // Exit code 4 means the child lost the lock race: another controller is
    // starting, so keep polling for a short grace period. If nobody answers
    // by then, the lock holder is not a starting daemon and waiting is futile.
    if (exitCode === 4) {
      lostRaceAt ??= Date.now();
      if (Date.now() - lostRaceAt > LOST_RACE_GRACE_MS)
        throw new ControllerUnavailableError(
          "start_failed",
          "another controller holds the project lock but does not answer; stop it or check the daemon log",
        );
    }
    if (exitCode !== undefined && exitCode !== 4)
      throw new ControllerUnavailableError(
        "start_failed",
        `the daemon exited during startup (exit code ${exitCode}): ${logTail(options.logPath, logOffset)}`,
      );
    const failed = unavailable(outcome);
    if (failed && outcome.outcome !== "unreachable") throw failed;
  }
  throw new ControllerUnavailableError(
    "start_timeout",
    `the daemon did not answer within ${(options.timeoutMs ?? START_TIMEOUT_MS) / 1000} seconds; see ${options.logPath} (another controller may be starting)`,
  );
}

export function logTail(logPath: string, fromOffset: number): string {
  try {
    const fd = fs.openSync(
      logPath,
      fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW,
    );
    try {
      const size = fs.fstatSync(fd).size;
      const start = Math.max(fromOffset, size - 4096);
      const buffer = Buffer.alloc(Math.max(0, size - start));
      fs.readSync(fd, buffer, 0, buffer.length, start);
      const lines = buffer
        .toString("utf8")
        .replace(/[\p{Cc}\p{Cf}]/gu, (ch) => (ch === "\n" ? "\n" : " "))
        .trimEnd()
        .split("\n");
      return (
        lines.slice(-3).join(" | ").slice(-400) || "(the daemon wrote nothing)"
      );
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return "(log unreadable)";
  }
}

function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error instanceof Error && "code" in error && error.code === "EPERM";
  }
}

export async function stopDaemon(
  socketPath: string,
  credential: string,
  timeoutMs = START_TIMEOUT_MS,
): Promise<"stopped" | "not_running"> {
  const first = await pingDaemon(socketPath, credential);
  if (first.outcome === "down") return "not_running";
  const blocked = unavailable(first);
  if (blocked) throw blocked;
  const pid = first.outcome === "running" ? first.pid : 0;
  const result = await callDaemon(socketPath, credential, "shutdown");
  if (result.kind !== "response" || !result.response.ok)
    throw new ControllerUnavailableError(
      "refused",
      "the daemon did not accept the shutdown request",
    );
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    // Stopped means the process is gone: the kernel then releases the project
    // lock. A closed listener alone is not enough, because the daemon frees
    // the lock a moment after it stops accepting connections.
    if (
      pid > 0
        ? !processAlive(pid)
        : (await pingDaemon(socketPath, credential)).outcome === "down"
    )
      return "stopped";
  }
  throw new ControllerUnavailableError(
    "start_timeout",
    "the daemon did not exit in time; check the pid file in the state directory",
  );
}

import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import { MAX_FRAME_BYTES, type CommandResponse } from "./daemon.js";

const DEFAULT_TIMEOUT_MS = 5_000;
const START_TIMEOUT_MS = 10_000;
const POLL_MS = 100;

export type WireResult =
  | { readonly kind: "response"; readonly response: CommandResponse }
  | { readonly kind: "legacy"; readonly body: Record<string, unknown> };

export class ControllerUnavailableError extends Error {
  override readonly name = "ControllerUnavailableError";
  constructor(
    readonly reason:
      "legacy" | "refused" | "unreachable" | "start_timeout" | "start_failed",
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
        if (bytes.length > 1_048_576)
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
        settle(() =>
          resolve(
            "ok" in record
              ? {
                  kind: "response",
                  response: record as unknown as CommandResponse,
                }
              : { kind: "legacy", body: record },
          ),
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
  | { readonly outcome: "legacy" }
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
    if (result.kind === "legacy") return { outcome: "legacy" };
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

export function openDaemonLog(logPath: string): number {
  const fd = fs.openSync(
    logPath,
    fs.constants.O_WRONLY |
      fs.constants.O_APPEND |
      fs.constants.O_CREAT |
      fs.constants.O_NOFOLLOW,
    0o600,
  );
  try {
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
  if (outcome.outcome === "legacy")
    return new ControllerUnavailableError(
      "legacy",
      "a foreground cstan run controller owns this project; stop it before using the daemon",
    );
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
  let exitCode: number | undefined;
  try {
    const child = spawn(
      process.execPath,
      [...process.execArgv, options.cliPath, "daemon"],
      {
        cwd: options.projectRoot,
        env: scrubEnvironment(options.env),
        detached: true,
        stdio: ["ignore", fd, fd],
      },
    );
    child.unref();
    child.once("exit", (code) => {
      exitCode = code ?? 1;
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
    // Exit code 4 means the child lost the lock race: another controller is
    // starting, so keep polling. Any other exit is a real startup failure.
    if (exitCode !== undefined && exitCode !== 4)
      throw new ControllerUnavailableError(
        "start_failed",
        `the daemon exited during startup (exit code ${exitCode}): ${logTail(options.logPath)}`,
      );
    const failed = unavailable(outcome);
    if (failed && outcome.outcome !== "unreachable") throw failed;
  }
  throw new ControllerUnavailableError(
    "start_timeout",
    `the daemon did not answer within ${(options.timeoutMs ?? START_TIMEOUT_MS) / 1000} seconds; see ${options.logPath} (another controller may be starting)`,
  );
}

function logTail(logPath: string): string {
  try {
    const text = fs.readFileSync(logPath, "utf8").trimEnd().split("\n");
    return text.slice(-3).join(" | ").slice(-400) || "(log is empty)";
  } catch {
    return "(log unreadable)";
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
  const result = await callDaemon(socketPath, credential, "shutdown");
  if (result.kind !== "response" || !result.response.ok)
    throw new ControllerUnavailableError(
      "refused",
      "the daemon did not accept the shutdown request",
    );
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    if ((await pingDaemon(socketPath, credential)).outcome === "down")
      return "stopped";
  }
  throw new ControllerUnavailableError(
    "start_timeout",
    "the daemon did not stop in time; check the pid file in the state directory",
  );
}

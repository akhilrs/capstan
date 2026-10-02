/**
 * Runs one shell command for the controller: `sh -c` in its own process group, a timeout that ends the
 * whole group, and a capped, sanitised tail of the output. The command, the working directory, the
 * environment and the timeout are spawn options; nothing is interpolated into a shell string.
 */
import { spawn } from "node:child_process";
import fs from "node:fs";
import { stripTerminalSequences } from "./observe.js";

export const RUN_KILL_GRACE_MS = 2000;
/** The raw window kept while a command runs; sanitising only shrinks it, so the final tail still has the requested size. */
const RAW_WINDOW_FACTOR = 4;
const MIN_RAW_WINDOW_BYTES = 16384;
const SECRET_NAME = /TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL/i;
const MIN_SECRET_VALUE_CHARS = 8;
export const REDACTED = "[redacted]";

/** Shapes that look like a credential: provider key prefixes, JWTs, bearer headers and long mixed letter-digit runs (a 40 or 64 hex commit or digest is left alone). */
const CREDENTIAL_PATTERNS: readonly RegExp[] = [
  /\b(?:Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{8,}/g,
  /\b(?:sk|pk|rk)-[A-Za-z0-9_-]{16,}/g,
  /\bgh[pousr]_[A-Za-z0-9]{20,}/g,
  /\bgithub_pat_[A-Za-z0-9_]{20,}/g,
  /\bxox[abprs]-[A-Za-z0-9-]{10,}/g,
  /\b(?:AKIA|ASIA)[A-Z0-9]{16}\b/g,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g,
  /-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)/g,
];
const LONG_RUN = /[A-Za-z0-9_-]{32,}/g;
const HEX_ONLY = /^[0-9a-fA-F]+$/;

function looksLikeSecretRun(run: string): boolean {
  return /[A-Za-z]/.test(run) && /[0-9]/.test(run) && !HEX_ONLY.test(run);
}

export interface SanitiseOptions {
  /** The most bytes of the result; the end of the text is kept. */
  readonly maxBytes: number;
  /** Values that must never appear, such as the values of TOKEN, SECRET, KEY and PASSWORD environment variables. */
  readonly secretValues?: readonly string[];
}

export interface SanitisedOutput {
  readonly text: string;
  readonly truncated: boolean;
}

/** The values of the environment variables whose names say they are secrets. */
export function secretValuesOf(
  ...environments: readonly NodeJS.ProcessEnv[]
): string[] {
  const values = new Set<string>();
  for (const environment of environments)
    for (const [name, value] of Object.entries(environment))
      if (
        SECRET_NAME.test(name) &&
        value !== undefined &&
        value.length >= MIN_SECRET_VALUE_CHARS
      )
        values.add(value);
  return [...values].sort((a, b) => b.length - a.length);
}

function tailOf(text: string, maxBytes: number): string {
  if (Buffer.byteLength(text, "utf8") <= maxBytes) return text;
  const segments = [
    ...new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(text),
  ].map((s) => s.segment);
  let bytes = 0;
  let start = segments.length;
  while (start > 0) {
    const size = Buffer.byteLength(segments[start - 1]!, "utf8");
    if (bytes + size > maxBytes) break;
    bytes += size;
    start -= 1;
  }
  return segments.slice(start).join("");
}

/**
 * Terminal escapes and C0 controls (other than newline and tab) are removed, credential shapes and the
 * given secret values are replaced by a marker, and the last `maxBytes` bytes are kept.
 */
export function sanitiseOutput(
  raw: string,
  options: SanitiseOptions,
): SanitisedOutput {
  let text = stripTerminalSequences(raw.toWellFormed())
    .replace(/\r\n?/g, "\n")
    .replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, "")
    .replace(/[\p{Cf}\p{Zl}\p{Zp}]/gu, "");
  for (const value of options.secretValues ?? [])
    if (value !== "") text = text.split(value).join(REDACTED);
  for (const pattern of CREDENTIAL_PATTERNS)
    text = text.replace(pattern, REDACTED);
  text = text.replace(LONG_RUN, (run) =>
    looksLikeSecretRun(run) ? REDACTED : run,
  );
  const kept = tailOf(text, options.maxBytes);
  return { text: kept, truncated: kept.length < text.length };
}

export type RunStatus = "ok" | "failed" | "timeout" | "error";

export interface RunResult {
  readonly status: RunStatus;
  readonly exitCode: number | null;
  readonly signal: string | null;
  readonly durationMs: number;
  readonly outputTail: string;
  /** True when output was dropped, either while the command ran or when the tail was cut. */
  readonly truncated: boolean;
}

export interface RunOptions {
  readonly command: string;
  readonly cwd: string;
  readonly timeoutMs: number;
  readonly environment: NodeJS.ProcessEnv;
  readonly outputTailBytes: number;
  /** Called once with the process group leader's pid as soon as the process exists. */
  readonly onSpawn?: (pid: number) => void;
  /** Aborting kills the process group like a timeout does; the status is then `error`. */
  readonly signal?: AbortSignal;
}

export function runCommand(options: RunOptions): Promise<RunResult> {
  const started = Date.now();
  const windowBytes = Math.max(
    MIN_RAW_WINDOW_BYTES,
    options.outputTailBytes * RAW_WINDOW_FACTOR,
  );
  const secrets = secretValuesOf(options.environment, process.env);
  return new Promise((resolve) => {
    let raw = Buffer.alloc(0);
    let dropped = false;
    const finish = (
      status: RunStatus,
      exitCode: number | null,
      signal: string | null,
      extra = "",
    ): void => {
      const text = raw.toString("utf8") + extra;
      const clean = sanitiseOutput(text, {
        maxBytes: options.outputTailBytes,
        secretValues: secrets,
      });
      resolve({
        status,
        exitCode,
        signal,
        durationMs: Date.now() - started,
        outputTail: clean.text,
        truncated: dropped || clean.truncated,
      });
    };
    let child;
    try {
      child = spawn("sh", ["-c", options.command], {
        cwd: options.cwd,
        detached: true,
        env: options.environment,
        stdio: ["ignore", "pipe", "pipe"],
      });
    } catch (error) {
      finish(
        "error",
        null,
        null,
        error instanceof Error ? error.message : String(error),
      );
      return;
    }
    const collect = (chunk: Buffer): void => {
      raw = Buffer.concat([raw, chunk]);
      if (raw.length > windowBytes) {
        raw = raw.subarray(raw.length - windowBytes);
        dropped = true;
      }
    };
    child.stdout.on("data", collect);
    child.stderr.on("data", collect);
    const killGroup = (signal: NodeJS.Signals): void => {
      try {
        if (child.pid !== undefined) process.kill(-child.pid, signal);
      } catch {
        // The group is already gone.
      }
    };
    let timedOut = false;
    let aborted = false;
    let hardKill: NodeJS.Timeout | undefined;
    const stopGroup = (): void => {
      killGroup("SIGTERM");
      hardKill ??= setTimeout(() => killGroup("SIGKILL"), RUN_KILL_GRACE_MS);
    };
    const timer = setTimeout(() => {
      timedOut = true;
      stopGroup();
    }, options.timeoutMs);
    const onAbort = (): void => {
      aborted = true;
      stopGroup();
    };
    if (options.signal?.aborted) onAbort();
    else options.signal?.addEventListener("abort", onAbort, { once: true });
    const cleanup = (): void => {
      clearTimeout(timer);
      clearTimeout(hardKill);
      options.signal?.removeEventListener("abort", onAbort);
    };
    child.on("error", (error) => {
      cleanup();
      finish("error", null, null, `\n${error.message}`);
    });
    child.on("close", (code, signal) => {
      cleanup();
      // A group member that ignored SIGTERM must still die.
      if (timedOut || aborted) killGroup("SIGKILL");
      if (timedOut) finish("timeout", code, signal);
      else if (aborted) finish("error", code, signal, "\nstopped");
      else if (code === 0) finish("ok", code, signal);
      else finish("failed", code, signal);
    });
    if (child.pid !== undefined) options.onSpawn?.(child.pid);
  });
}

/** The start time field of /proc/<pid>/stat (clock ticks since boot), or null when the process does not exist or the file cannot be read. It tells a live leader from a pid that was reused. */
export function processStartTime(pid: number): string | null {
  try {
    const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8");
    // The command name is in parentheses and may contain spaces or parentheses itself.
    const afterName = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
    const start = afterName[19];
    return start !== undefined && /^\d+$/.test(start) ? start : null;
  } catch {
    return null;
  }
}

/** Whether a process group still has a member. */
export function groupIsAlive(pgid: number): boolean {
  try {
    process.kill(-pgid, 0);
    return true;
  } catch (error) {
    return !(
      error instanceof Error &&
      "code" in error &&
      error.code === "ESRCH"
    );
  }
}

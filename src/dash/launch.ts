import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isSea } from "../sea.js";

export const DASH_BIN_NAME = "cstan-dash";

export const NODE_DASH_HINT =
  "cstan: using the Node dashboard; install cstan-dash for lower CPU and memory: sh install.sh, or npm run build:dash in a source checkout (docs/reference/install.md)";

export interface LaunchOptions {
  readonly intervalSeconds: number;
  readonly noColor: boolean;
  readonly reducedMotion: boolean;
}

export interface LaunchRuntime {
  readonly socketPath: string;
  readonly credential: string;
  readonly workerLimit: number | null;
}

/** Everything the resolver reads from the machine, so tests can place fake binaries anywhere. */
export interface ResolveContext {
  readonly env: NodeJS.ProcessEnv;
  /** The running cstan executable when it is the standalone binary, else null. */
  readonly seaExecutable: string | null;
  /** The directory of this module (dist/src/dash when running from a build). */
  readonly moduleDir: string;
  readonly home: string;
  /** Whether a candidate really runs as cstan-dash; defaults to running `<bin> --version`. */
  readonly probe?: (bin: string) => boolean;
}

export type Resolution =
  | { readonly kind: "node" }
  | { readonly kind: "rust"; readonly bin: string }
  | { readonly kind: "none" };

export function isExecutableFile(file: string): boolean {
  try {
    if (!fs.statSync(file).isFile()) return false;
    fs.accessSync(file, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function realOrSelf(file: string): string {
  try {
    return fs.realpathSync(file);
  } catch {
    return file;
  }
}

/**
 * process.execve aborts the whole process (exit 134, not catchable) for a file that looks like an ELF but cannot run,
 * so a candidate must prove it runs before it is trusted: `--version` printing `cstan-dash <version>`.
 */
export function probeDash(bin: string): boolean {
  const result = spawnSync(bin, ["--version"], {
    timeout: 2000,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  return (
    result.error === undefined &&
    result.status === 0 &&
    /^cstan-dash \S+/.test(result.stdout)
  );
}

/** The candidate paths in search order (override first); the caller takes the first executable file. */
export function candidatePaths(ctx: ResolveContext): string[] {
  const out: string[] = [];
  const override = ctx.env.CSTAN_DASH_BIN ?? "";
  if (override !== "" && path.isAbsolute(override)) out.push(override);
  if (ctx.seaExecutable !== null)
    out.push(
      path.join(path.dirname(realOrSelf(ctx.seaExecutable)), DASH_BIN_NAME),
    );
  // dist/src/dash -> the repository root is three levels up; only a build tree has that shape.
  const parts = ctx.moduleDir.split(path.sep);
  if (parts.slice(-3, -1).join("/") === "dist/src") {
    const root = path.resolve(ctx.moduleDir, "..", "..", "..");
    out.push(path.join(root, "dash", "target", "release", DASH_BIN_NAME));
  }
  const data = ctx.env.XDG_DATA_HOME
    ? ctx.env.XDG_DATA_HOME
    : path.join(ctx.home, ".local", "share");
  out.push(path.join(data, "capstan", "current", "bin", DASH_BIN_NAME));
  for (const dir of (ctx.env.PATH ?? "").split(path.delimiter))
    if (dir !== "" && path.isAbsolute(dir))
      out.push(path.join(dir, DASH_BIN_NAME));
  return out;
}

/** CSTAN_DASH=node never looks; rust demands a binary; anything else searches and may find none. */
export function resolveDash(ctx: ResolveContext): Resolution {
  const mode = ctx.env.CSTAN_DASH ?? "";
  if (mode === "node") return { kind: "node" };
  const probe = ctx.probe ?? probeDash;
  let broken: string | null = null;
  for (const candidate of candidatePaths(ctx)) {
    if (!isExecutableFile(candidate)) continue;
    if (probe(candidate)) return { kind: "rust", bin: candidate };
    broken ??= candidate;
  }
  if (mode === "rust")
    throw new Error(
      broken !== null
        ? `CSTAN_DASH=rust but ${broken} does not run as ${DASH_BIN_NAME} (\`--version\` failed); rebuild or reinstall it (docs/reference/install.md)`
        : `CSTAN_DASH=rust but no executable ${DASH_BIN_NAME} was found; set CSTAN_DASH_BIN to its absolute path or install it (docs/reference/install.md)`,
    );
  return { kind: "none" };
}

export function defaultContext(): ResolveContext {
  return {
    env: process.env,
    seaExecutable: isSea() ? process.execPath : null,
    moduleDir: path.dirname(fileURLToPath(import.meta.url)),
    home: os.homedir(),
  };
}

export function dashArgs(
  options: LaunchOptions,
  runtime: LaunchRuntime,
): string[] {
  const args = [
    "--socket",
    runtime.socketPath,
    "--interval",
    String(options.intervalSeconds),
  ];
  if (runtime.workerLimit !== null)
    args.push("--worker-limit", String(runtime.workerLimit));
  if (options.noColor) args.push("--no-color");
  if (options.reducedMotion) args.push("--reduced-motion");
  return args;
}

/** The child's environment: no agent credentials, and the operator credential under its own name. */
export function dashEnv(
  env: NodeJS.ProcessEnv,
  credential: string,
): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(env))
    if (
      value !== undefined &&
      key !== "CAPSTAN_TOKEN" &&
      key !== "CAPSTAN_SOCKET"
    )
      out[key] = value;
  out.CSTAN_DASH_CREDENTIAL = credential;
  return out;
}

/**
 * process.execve aborts the whole process, without a catchable error, when the exec fails. Only an ELF file, or a
 * script whose shebang interpreter is itself executable, is safe to hand to it; anything else goes through spawn,
 * which reports ENOENT/EACCES/ENOEXEC as an error event.
 */
export function safeToExecve(file: string): boolean {
  const format = readFormat(file);
  if (format.kind === "elf") return true;
  return (
    format.kind === "script" &&
    path.isAbsolute(format.interpreter) &&
    isExecutableFile(format.interpreter)
  );
}

type Format =
  | { readonly kind: "elf" }
  | { readonly kind: "script"; readonly interpreter: string }
  | { readonly kind: "unknown" };

/** What kind of program a file is. Anything else would be run by libuv through /bin/sh, which cstan-dash never is. */
function readFormat(file: string): Format {
  let head: Buffer;
  try {
    const fd = fs.openSync(file, "r");
    try {
      head = Buffer.alloc(256);
      head = head.subarray(0, fs.readSync(fd, head, 0, 256, 0));
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return { kind: "unknown" };
  }
  if (head.length >= 4 && head.readUInt32BE(0) === 0x7f454c46)
    return { kind: "elf" };
  if (head.length >= 2 && head[0] === 0x23 && head[1] === 0x21) {
    const line = (head.toString("latin1").split("\n")[0] ?? "").slice(2).trim();
    return { kind: "script", interpreter: line.split(/\s+/)[0] ?? "" };
  }
  return { kind: "unknown" };
}

export interface LaunchDeps {
  /** Replaces the process; absent where the runtime lacks it. */
  readonly execve:
    | ((file: string, args: string[], env: Record<string, string>) => never)
    | undefined;
  readonly spawnChild: typeof spawn;
  readonly onSignal: (
    signal: NodeJS.Signals,
    handler: () => void,
  ) => () => void;
}

export function defaultLaunchDeps(): LaunchDeps {
  const execve = process.execve;
  return {
    execve:
      typeof execve === "function"
        ? (file, args, env) => execve.call(process, file, args, env)
        : undefined,
    spawnChild: spawn,
    onSignal: (signal, handler) => {
      process.on(signal, handler);
      return () => void process.off(signal, handler);
    },
  };
}

const FALLBACK_CODES = new Set(["ENOENT", "EACCES", "ENOEXEC"]);

export type LaunchOutcome =
  | { readonly kind: "exited"; readonly code: number }
  | { readonly kind: "unusable" };

/**
 * Runs cstan-dash in place of this process (execve) or as a child (spawn, stdio inherited). `unusable` means the
 * binary could not be started and the caller should run the Node dashboard.
 */
export async function launchRust(
  bin: string,
  options: LaunchOptions,
  runtime: LaunchRuntime,
  env: NodeJS.ProcessEnv,
  deps: LaunchDeps,
): Promise<LaunchOutcome> {
  if (readFormat(bin).kind === "unknown") return { kind: "unusable" };
  const args = [bin, ...dashArgs(options, runtime)];
  const childEnv = dashEnv(env, runtime.credential);
  const execve = deps.execve;
  if (execve !== undefined && safeToExecve(bin)) {
    try {
      execve(bin, args, childEnv);
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code ?? "";
      if (FALLBACK_CODES.has(code)) return { kind: "unusable" };
      // Any other exec failure: try again as a child process.
    }
  }
  return await new Promise<LaunchOutcome>((resolve) => {
    const child = deps.spawnChild(bin, args.slice(1), {
      stdio: "inherit",
      env: childEnv,
    });
    const offs = (["SIGTERM", "SIGHUP"] as const).map((signal) =>
      deps.onSignal(signal, () => void child.kill(signal)),
    );
    const done = (outcome: LaunchOutcome): void => {
      for (const off of offs) off();
      resolve(outcome);
    };
    child.once("error", (error: NodeJS.ErrnoException) => {
      done(
        FALLBACK_CODES.has(error.code ?? "")
          ? { kind: "unusable" }
          : { kind: "exited", code: 1 },
      );
    });
    child.once("close", (code, signal) => {
      done({
        kind: "exited",
        code: code ?? (signal ? 128 + (os.constants.signals[signal] ?? 0) : 1),
      });
    });
  });
}

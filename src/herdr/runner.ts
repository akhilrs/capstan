import { spawn } from "node:child_process";

const MAX_OUTPUT_BYTES = 4 * 1024 * 1024;
const SESSION_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

export interface HerdrResult {
  readonly code: number;
  readonly stdout: string;
  readonly stderr: string;
}

export interface RunOptions {
  /** Overrides the runner's own limit for one call, for commands that wait on Herdr by design. */
  readonly timeoutMs?: number;
}

export type HerdrRunner = (
  args: readonly string[],
  options?: RunOptions,
) => Promise<HerdrResult>;

export class HerdrError extends Error {
  override readonly name = "HerdrError";
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** The environment with every HERDR_ variable removed, so a command can never follow the caller's own pane into another session. */
export function herdrEnvironment(base: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const copy: NodeJS.ProcessEnv = {};
  for (const [name, value] of Object.entries(base))
    if (!name.startsWith("HERDR_")) copy[name] = value;
  return copy;
}

export interface RunnerOptions {
  /** Required: the session is always named, so the operator's session cannot be reached by omission or inheritance. */
  readonly session: string;
  readonly binary?: string;
  readonly timeoutMs?: number;
  readonly env?: NodeJS.ProcessEnv;
}

export function createHerdrRunner(options: RunnerOptions): HerdrRunner {
  if (
    typeof options.session !== "string" ||
    !SESSION_PATTERN.test(options.session)
  )
    throw new TypeError("a Herdr session name is required");
  const binary = options.binary ?? "herdr";
  const timeoutMs = options.timeoutMs ?? 30_000;
  const environment = herdrEnvironment(options.env ?? process.env);
  return (args, callOptions) =>
    new Promise<HerdrResult>((resolve, reject) => {
      const child = spawn(binary, ["--session", options.session, ...args], {
        env: environment,
        stdio: ["ignore", "pipe", "pipe"],
      });
      const stdoutChunks: Buffer[] = [];
      const stderrChunks: Buffer[] = [];
      let size = 0;
      let failure: Error | undefined;
      const stop = (error: Error): void => {
        failure ??= error;
        child.kill("SIGKILL");
      };
      const timer = setTimeout(
        () =>
          stop(new HerdrError("timeout", `herdr ${args[0] ?? ""} timed out`)),
        callOptions?.timeoutMs ?? timeoutMs,
      );
      const collect =
        (chunks: Buffer[]) =>
        (chunk: Buffer): void => {
          size += chunk.length;
          if (size > MAX_OUTPUT_BYTES)
            stop(
              new HerdrError(
                "output_too_large",
                "herdr output exceeded the limit",
              ),
            );
          else chunks.push(chunk);
        };
      child.stdout.on("data", collect(stdoutChunks));
      child.stderr.on("data", collect(stderrChunks));
      child.once("error", (error) => {
        clearTimeout(timer);
        reject(error);
      });
      child.once("close", (code) => {
        clearTimeout(timer);
        if (failure) reject(failure);
        else
          resolve({
            code: code ?? 1,
            stdout: Buffer.concat(stdoutChunks).toString("utf8"),
            stderr: Buffer.concat(stderrChunks).toString("utf8"),
          });
      });
    });
}

/** Control characters and length are removed so Herdr output cannot inject escape sequences into logs. */
export function describeOutput(text: string): string {
  return (
    Array.from(text.replace(/[\p{Cc}\p{Cf}]/gu, " ").trim())
      .slice(0, 200)
      .join("") || "no output"
  );
}

function errorFrom(text: string): HerdrError | undefined {
  try {
    const body: unknown = JSON.parse(text.replace(/^\uFEFF/, ""));
    if (typeof body !== "object" || body === null) return undefined;
    const error = (body as { error?: unknown }).error;
    if (typeof error !== "object" || error === null) return undefined;
    const details = error as { code?: unknown; message?: unknown };
    return new HerdrError(
      typeof details.code === "string" &&
        /^[A-Za-z0-9_.-]{1,64}$/.test(details.code)
        ? details.code
        : "error",
      typeof details.message === "string"
        ? describeOutput(details.message)
        : "herdr failed",
    );
  } catch {
    return undefined;
  }
}

/** The failure a non-zero exit stands for. Herdr writes it as JSON on stderr, and older shapes used stdout. */
export function failureOf(
  args: readonly string[],
  outcome: HerdrResult,
): HerdrError {
  return (
    errorFrom(outcome.stderr) ??
    errorFrom(outcome.stdout) ??
    new HerdrError(
      "exit",
      `herdr ${args[0] ?? ""} failed (exit ${outcome.code}): ${describeOutput(outcome.stderr)}`,
    )
  );
}

/** Runs a Herdr command that answers with `{id, result}` JSON and returns the result, or throws the failure as a HerdrError. */
export async function runJson(
  runner: HerdrRunner,
  args: readonly string[],
  options?: RunOptions,
): Promise<Record<string, unknown>> {
  const outcome = await runner(args, options);
  if (outcome.code !== 0) throw failureOf(args, outcome);
  let body: unknown;
  try {
    body = JSON.parse(outcome.stdout.replace(/^\uFEFF/, ""));
  } catch {
    throw new HerdrError(
      "bad_output",
      `herdr ${args[0] ?? ""} did not answer with JSON: ${describeOutput(outcome.stdout)}`,
    );
  }
  if (typeof body !== "object" || body === null || Array.isArray(body))
    throw new HerdrError(
      "bad_output",
      "herdr answered with an unexpected shape",
    );
  const record = body as Record<string, unknown>;
  const error = errorFrom(outcome.stdout);
  if (error !== undefined) throw error;
  const result = record.result;
  if (typeof result !== "object" || result === null || Array.isArray(result))
    throw new HerdrError("bad_output", "herdr answered without a result");
  return result as Record<string, unknown>;
}

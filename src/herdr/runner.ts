import { spawn } from "node:child_process";

const MAX_OUTPUT_BYTES = 4 * 1024 * 1024;
const SESSION_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

export interface HerdrResult {
  readonly code: number;
  readonly stdout: string;
  readonly stderr: string;
}

export type HerdrRunner = (args: readonly string[]) => Promise<HerdrResult>;

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
  return (args) =>
    new Promise<HerdrResult>((resolve, reject) => {
      const child = spawn(binary, ["--session", options.session, ...args], {
        env: environment,
        stdio: ["ignore", "pipe", "pipe"],
      });
      let stdout = "";
      let stderr = "";
      let size = 0;
      let failure: Error | undefined;
      const stop = (error: Error): void => {
        failure ??= error;
        child.kill("SIGKILL");
      };
      const timer = setTimeout(
        () =>
          stop(new HerdrError("timeout", `herdr ${args[0] ?? ""} timed out`)),
        timeoutMs,
      );
      const collect =
        (append: (text: string) => void) =>
        (chunk: Buffer): void => {
          size += chunk.length;
          if (size > MAX_OUTPUT_BYTES)
            stop(
              new HerdrError(
                "output_too_large",
                "herdr output exceeded the limit",
              ),
            );
          else append(chunk.toString("utf8"));
        };
      child.stdout.on(
        "data",
        collect((text) => {
          stdout += text;
        }),
      );
      child.stderr.on(
        "data",
        collect((text) => {
          stderr += text;
        }),
      );
      child.once("error", (error) => {
        clearTimeout(timer);
        reject(error);
      });
      child.once("close", (code) => {
        clearTimeout(timer);
        if (failure) reject(failure);
        else resolve({ code: code ?? 1, stdout, stderr });
      });
    });
}

/** Runs a Herdr command that answers with `{id, result}` JSON and returns the result, or throws the `{error}` as a HerdrError. */
export async function runJson(
  runner: HerdrRunner,
  args: readonly string[],
): Promise<Record<string, unknown>> {
  const outcome = await runner(args);
  let body: unknown;
  try {
    // Herdr reports a failed command as JSON on stderr with a non-zero exit.
    body = JSON.parse(
      outcome.stdout.trim() === "" ? outcome.stderr : outcome.stdout,
    );
  } catch {
    throw new HerdrError(
      "bad_output",
      `herdr ${args[0] ?? ""} did not answer with JSON (exit ${outcome.code}): ${
        outcome.stderr
          .replace(/\p{Cc}/gu, " ")
          .trim()
          .slice(0, 200) || "no error output"
      }`,
    );
  }
  if (typeof body !== "object" || body === null || Array.isArray(body))
    throw new HerdrError(
      "bad_output",
      "herdr answered with an unexpected shape",
    );
  const record = body as Record<string, unknown>;
  const error = record.error;
  if (typeof error === "object" && error !== null) {
    const details = error as { code?: unknown; message?: unknown };
    throw new HerdrError(
      typeof details.code === "string" ? details.code : "error",
      typeof details.message === "string" ? details.message : "herdr failed",
    );
  }
  const result = record.result;
  if (typeof result !== "object" || result === null || Array.isArray(result))
    throw new HerdrError("bad_output", "herdr answered without a result");
  return result as Record<string, unknown>;
}

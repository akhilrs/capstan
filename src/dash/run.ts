import { callDaemon } from "../client.js";
import type { CallResult } from "./app.js";
import { wantsAscii } from "./terminal.js";

export interface DashOptions {
  readonly intervalSeconds: number;
  readonly noColor: boolean;
  readonly reducedMotion: boolean;
}

export interface DashRuntime {
  readonly socketPath: string;
  readonly credential: string;
  /** `limits.max_workers` from capstan.toml, or null when the config cannot be read. */
  readonly workerLimit: number | null;
}

/** Runs the dashboard until the operator quits. The caller has already checked for a terminal and started the daemon. */
export async function runDash(
  options: DashOptions,
  runtime: DashRuntime,
): Promise<void> {
  // Ink's colour support is decided when it loads, so the flag must be set first.
  if (options.noColor) process.env.NO_COLOR = "1";
  const { createElement } = await import("react");
  const { render } = await import("ink");
  const { makeTheme } = await import("./theme.js");
  const { App } = await import("./app.js");
  const theme = makeTheme({
    noColor:
      options.noColor ||
      (process.env.NO_COLOR ?? "") !== "" ||
      process.env.TERM === "dumb",
    reducedMotion: options.reducedMotion,
    ascii: wantsAscii(process.env),
  });
  const fetchStatus = async (): Promise<Record<string, unknown>> => {
    const result = await callDaemon(
      runtime.socketPath,
      runtime.credential,
      "status",
    );
    if (!result.response.ok) throw new Error("status is unavailable");
    return result.response.result as Record<string, unknown>;
  };
  const call = async (
    command: string,
    args: readonly string[],
  ): Promise<CallResult> => {
    try {
      const result = await callDaemon(
        runtime.socketPath,
        runtime.credential,
        command,
        args,
      );
      return result.response.ok
        ? { ok: true, result: result.response.result }
        : { ok: false, message: result.response.message };
    } catch {
      return { ok: false, message: "the controller did not answer" };
    }
  };
  const app = render(
    createElement(App, {
      deps: {
        fetch: fetchStatus,
        call,
        workerLimit: runtime.workerLimit,
        intervalSeconds: options.intervalSeconds,
        theme,
      },
    }),
    { alternateScreen: true, exitOnCtrlC: false, incrementalRendering: true },
  );
  await app.waitUntilExit();
}

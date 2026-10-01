import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { randomBytes } from "node:crypto";
import { HerdrAdapter } from "../src/herdr/adapter.js";
import { createHerdrRunner, type HerdrRunner } from "../src/herdr/runner.js";

const MAX_TEMP_PATH = 70;

export interface LiveEnvironment {
  readonly session: string;
  readonly home: string;
  readonly repo: string;
  readonly root: string;
  readonly binDirectory: string;
  readonly argsLog: string;
  readonly messageLog: string;
  readonly adapter: HerdrAdapter;
  readonly runner: HerdrRunner;
  readonly serverEnvironment: NodeJS.ProcessEnv;
  cleanup(): Promise<void>;
}

/** Returns why the live harness cannot run, or undefined when it can. */
export function liveUnavailable(): string | undefined {
  for (const [binary, args] of [
    ["herdr", ["--version"]],
    ["python3", ["--version"]],
    ["git", ["--version"]],
  ] as const)
    try {
      execFileSync(binary, args, { stdio: "ignore" });
    } catch {
      return `${binary} is not available`;
    }
  return undefined;
}

function isolatedEnvironment(
  home: string,
  binDirectory: string,
): NodeJS.ProcessEnv {
  const environment: NodeJS.ProcessEnv = {};
  for (const [name, value] of Object.entries(process.env)) {
    if (value === undefined) continue;
    if (name.startsWith("HERDR_") || name.startsWith("XDG_")) continue;
    environment[name] = value;
  }
  environment.HOME = home;
  environment.PATH = `${binDirectory}:${process.env.PATH ?? ""}`;
  environment.TERM = "xterm-256color";
  return environment;
}

/** Lists the operator's default session without changing it, with the real HOME. Focus, agent status and pane counts follow the operator's own use of the terminal while a test runs, so only each workspace's id and label are compared: a workspace the code opened or closed in the default session still shows. */
export function defaultSessionSnapshot(): string {
  const environment: NodeJS.ProcessEnv = {};
  for (const [name, value] of Object.entries(process.env))
    if (value !== undefined && !name.startsWith("HERDR_"))
      environment[name] = value;
  try {
    const listing = execFileSync(
      "herdr",
      ["--session", "default", "workspace", "list"],
      {
        env: environment,
        encoding: "utf8",
        timeout: 15_000,
      },
    );
    const parsed = JSON.parse(listing) as {
      result?: { workspaces?: Array<Record<string, unknown>> };
    };
    return JSON.stringify(
      (parsed.result?.workspaces ?? []).map((workspace) =>
        Object.fromEntries(
          Object.entries(workspace).filter(
            ([key]) => key === "workspace_id" || key === "label",
          ),
        ),
      ),
    );
  } catch {
    return "unavailable";
  }
}

export async function startLiveEnvironment(): Promise<LiveEnvironment> {
  const root = realpathSync(mkdtempSync("/tmp/cph-"));
  if (root.length > MAX_TEMP_PATH) throw new Error("temporary path too long");
  const home = path.join(root, "home");
  const binDirectory = path.join(root, "bin");
  mkdirSync(home);
  mkdirSync(binDirectory);
  for (const file of [".bashrc", ".bash_profile"])
    writeFileSync(path.join(home, file), "PS1='❯ '\n");
  const python = execFileSync(
    "python3",
    ["-c", "import sys;print(sys.executable)"],
    { encoding: "utf8" },
  ).trim();
  const fake = readFileSync(
    path.resolve("test/fixtures/fake-claude.py"),
    "utf8",
  );
  const wrapper = path.join(binDirectory, "claude");
  writeFileSync(wrapper, `#!${python}\n${fake.replace(/^#!.*\n/, "")}`);
  chmodSync(wrapper, 0o755);
  // An agent started through the launcher gets a scrubbed environment, so the
  // stand-in finds its screens beside itself instead of through a variable.
  copyFileSync(
    path.resolve("test/fixtures/claude-trust-dialog.txt"),
    path.join(binDirectory, "claude-trust-dialog.txt"),
  );

  const session = `capstan-test-${randomBytes(4).toString("hex")}`;
  const serverEnvironment = isolatedEnvironment(home, binDirectory);
  const argsLog = path.join(root, "args.log");
  const messageLog = path.join(root, "messages.log");
  serverEnvironment.FAKE_CLAUDE_FIXTURES = path.resolve("test/fixtures");
  serverEnvironment.FAKE_CLAUDE_ARGS = argsLog;
  serverEnvironment.FAKE_CLAUDE_LOG = messageLog;

  const repo = path.join(root, "repo");
  mkdirSync(repo);
  for (const args of [
    ["init", "-q", "-b", "main"],
    [
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@example.com",
      "commit",
      "-q",
      "--allow-empty",
      "-m",
      "init",
    ],
  ])
    execFileSync("git", args, { cwd: repo });

  const server: ChildProcess = spawn(
    "herdr",
    ["--session", session, "server"],
    { env: serverEnvironment, stdio: "ignore", detached: true },
  );
  server.unref();
  const runner = createHerdrRunner({
    session,
    env: serverEnvironment,
    timeoutMs: 30_000,
  });
  const deadline = Date.now() + 20_000;
  for (;;) {
    const result = await runner(["workspace", "list"]).catch(() => undefined);
    if (result?.code === 0) break;
    if (Date.now() > deadline) {
      await teardown();
      throw new Error("the isolated Herdr server did not start");
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const adapter = new HerdrAdapter({ run: runner, tempRoot: root });

  async function teardown(): Promise<void> {
    try {
      execFileSync("herdr", ["--session", session, "server", "stop"], {
        env: serverEnvironment,
        stdio: "ignore",
        timeout: 15_000,
      });
    } catch {
      if (server.pid !== undefined)
        try {
          process.kill(server.pid);
        } catch {
          /* already gone */
        }
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
    try {
      execFileSync("herdr", ["session", "delete", session], {
        env: serverEnvironment,
        stdio: "ignore",
        timeout: 15_000,
      });
    } catch {
      /* the session may already be gone */
    }
    rmSync(root, { recursive: true, force: true });
  }

  return {
    session,
    home,
    repo,
    root,
    binDirectory,
    argsLog,
    messageLog,
    adapter,
    runner,
    serverEnvironment,
    cleanup: async () => {
      adapter.close();
      await teardown();
    },
  };
}

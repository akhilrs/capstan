/**
 * Opt-in live check of the Researcher role: a real Claude Code, a real Herdr session, the real
 * network and `npx @playwright/mcp`. It runs only when CAPSTAN_LIVE_RESEARCHER=1 and is not part of
 * the CI pass. It automates steps 1 to 3 of docs/researcher-live-evidence.txt: the config check, the
 * spawn with the playwright MCP server connected, and a curl pipeline that runs with no permission
 * prompt. Reddit may answer with a login redirect, 403 or 429; that still proves the curl path.
 */
import assert from "node:assert/strict";
import {
  execFileSync,
  spawn,
  spawnSync,
  type ChildProcess,
} from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  existsSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { randomBytes } from "node:crypto";

const FLAG = process.env.CAPSTAN_LIVE_RESEARCHER === "1";
const CLI = path.resolve("dist/src/cli.js");
const BLOCKS = new Set(["# [researcher]", "# [roles.researcher]"]);

function unavailable(): string | undefined {
  if (!FLAG) return "set CAPSTAN_LIVE_RESEARCHER=1 to run";
  for (const [binary, args] of [
    ["herdr", ["--version"]],
    ["claude", ["--version"]],
    ["git", ["--version"]],
    ["jq", ["--version"]],
    ["npx", ["--version"]],
  ] as const)
    try {
      execFileSync(binary, args, { stdio: "ignore" });
    } catch {
      return `${binary} is not available`;
    }
  const realHome = homedir();
  for (const needed of [".claude", ".claude.json"])
    if (!existsSync(path.join(realHome, needed)))
      return `${needed} is missing, so Claude Code is not signed in`;
  return undefined;
}

const UNAVAILABLE = unavailable();
if (UNAVAILABLE !== undefined && FLAG)
  console.error(`SKIPPED LOUDLY: live researcher test: ${UNAVAILABLE}`);

/** Uncomments the three researcher blocks of the starter template: a header line and the key lines that follow it up to the next blank line. */
function enableResearcher(template: string, session: string): string {
  let on = false;
  const lines = template.split("\n").map((line) => {
    if (BLOCKS.has(line)) on = true;
    else if (on && !line.startsWith("# ")) on = false;
    return on ? line.slice(2) : line;
  });
  return `herdr_session = "${session}"\n${lines.join("\n")}`;
}

test(
  "a spawned researcher has the playwright MCP connected and runs a read-only curl pipeline with no permission prompt",
  { skip: UNAVAILABLE, timeout: 600_000 },
  async () => {
    const root = realpathSync(mkdtempSync("/tmp/cph-res-"));
    const home = path.join(root, "home");
    const project = path.join(root, "project");
    const session = `capstan-res-${randomBytes(4).toString("hex")}`;
    mkdirSync(home);
    mkdirSync(path.join(home, ".cache"), { recursive: true });
    mkdirSync(project);
    // A bare `❯ ` prompt is what Capstan types into; the operator's real prompt is not. Claude's
    // sign-in and the Playwright browsers are shared by link, never copied.
    for (const file of [".bashrc", ".bash_profile"])
      writeFileSync(path.join(home, file), "PS1='❯ '\n");
    const real = homedir();
    symlinkSync(path.join(real, ".claude"), path.join(home, ".claude"));
    symlinkSync(
      path.join(real, ".claude.json"),
      path.join(home, ".claude.json"),
    );
    const browsers = path.join(real, ".cache", "ms-playwright");
    if (existsSync(browsers))
      symlinkSync(browsers, path.join(home, ".cache", "ms-playwright"));
    const environment: NodeJS.ProcessEnv = {};
    for (const [name, value] of Object.entries(process.env))
      if (value !== undefined && !/^(HERDR_|CAPSTAN_|XDG_)/.test(name))
        environment[name] = value;
    Object.assign(environment, {
      HOME: home,
      SHELL: "/bin/bash",
      TERM: "xterm-256color",
    });
    const cstan = (...args: string[]) =>
      spawnSync("node", [CLI, ...args], {
        cwd: project,
        env: environment,
        encoding: "utf8",
        timeout: 180_000,
      });
    const herdr = (...args: string[]): string =>
      execFileSync("herdr", ["--session", session, ...args], {
        env: environment,
        encoding: "utf8",
        timeout: 30_000,
      });
    const sleep = (ms: number) =>
      new Promise((resolve) => setTimeout(resolve, ms));
    const screenWhen = async (
      pane: string,
      done: (screen: string) => boolean,
      what: string,
      timeoutMs = 120_000,
    ): Promise<string> => {
      const deadline = Date.now() + timeoutMs;
      for (;;) {
        const screen = herdr("pane", "read", pane);
        if (done(screen)) return screen;
        if (Date.now() > deadline)
          throw new Error(`timed out waiting for ${what}:\n${screen}`);
        await sleep(2_000);
      }
    };
    let server: ChildProcess | undefined;
    try {
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
        execFileSync("git", args, { cwd: project });
      // Step 1: the starter template with the researcher blocks uncommented passes the config check.
      const init = cstan("init");
      assert.equal(init.status, 0, init.stderr);
      const file = path.join(project, "capstan.toml");
      writeFileSync(
        file,
        enableResearcher(readFileSync(file, "utf8"), session),
      );
      const check = cstan("config", "check");
      assert.equal(check.status, 0, check.stderr);
      const resolved = JSON.parse(check.stdout) as {
        researcher: { enabled: boolean };
        mcpServers: { name: string }[];
      };
      assert.equal(resolved.researcher.enabled, true);
      assert.deepEqual(
        resolved.mcpServers.map((s) => s.name),
        ["playwright"],
      );

      // Step 2: spawn the researcher; Claude Code lists the playwright server as connected.
      server = spawn("herdr", ["--session", session, "server"], {
        env: environment,
        stdio: "ignore",
        detached: true,
      });
      server.unref();
      for (let i = 0; ; i++) {
        try {
          herdr("workspace", "list");
          break;
        } catch (error) {
          if (i > 100) throw error;
          await sleep(200);
        }
      }
      const started = cstan("start");
      assert.match(started.stdout, /state: blocked/, started.stdout);
      const pm = /paneId: (\S+)/.exec(started.stdout)![1]!;
      await screenWhen(pm, (s) => s.includes("trust this folder"), "PM dialog");
      herdr("pane", "send-keys", pm, "down");
      herdr("pane", "send-keys", pm, "enter");
      const spawned = cstan("spawn", "researcher");
      assert.equal(spawned.status, 0, spawned.stdout + spawned.stderr);
      assert.match(spawned.stdout, /state: started/);
      const pane = /paneId: (\S+)/.exec(spawned.stdout)![1]!;
      await screenWhen(
        pane,
        (s) => s.includes("manual mode on"),
        "the researcher prompt",
      );
      herdr("pane", "send-text", pane, "/mcp");
      herdr("pane", "send-keys", pane, "enter");
      await screenWhen(
        pane,
        (s) => /✔ playwright/.test(s),
        "the playwright MCP server to show as connected (the first run downloads it through npx)",
        300_000,
      );
      herdr("pane", "send-keys", pane, "escape");
      await sleep(2_000);

      // Step 3: the curl pipeline runs with no permission prompt. A login redirect, 403 or 429 from Reddit leaves empty output and still counts.
      herdr(
        "pane",
        "send-text",
        pane,
        "Run exactly this one shell command and show its output: curl -sS -A 'capstan-researcher/1.0 (research bot; contact: project owner)' 'https://old.reddit.com/r/ClaudeAI/search.json?q=design&restrict_sr=1&limit=3' | jq '.data.children[].data.title'",
      );
      await sleep(1_000);
      herdr("pane", "send-keys", pane, "enter");
      const screen = await screenWhen(
        pane,
        (s) =>
          /Ran 1 shell command|Permission to use|Do you want to proceed/.test(
            s,
          ),
        "the curl command to run",
      );
      assert.ok(!/Do you want to proceed/.test(screen), screen);
      assert.ok(!/Permission to use Bash/.test(screen), screen);
      assert.match(screen, /Ran 1 shell command/);
      // Step 4: a browser visit works and leaves no .playwright-mcp directory in the worktree.
      const worktree = /worktreePath: (\S+)/.exec(spawned.stdout)![1]!;
      herdr(
        "pane",
        "send-text",
        pane,
        "Use browser_navigate to load https://news.ycombinator.com, then browser_snapshot, and name the first story.",
      );
      await sleep(1_000);
      herdr("pane", "send-keys", pane, "enter");
      await screenWhen(
        pane,
        (s) => /Called playwright 2 times/.test(s),
        "the browser visit",
      );
      assert.ok(
        !existsSync(path.join(worktree, ".playwright-mcp")),
        "no .playwright-mcp directory in the worktree",
      );
    } finally {
      cstan("stop");
      try {
        herdr("server", "stop");
      } catch {
        if (server?.pid !== undefined)
          try {
            process.kill(server.pid);
          } catch {
            /* already gone */
          }
      }
      await sleep(500);
      try {
        execFileSync("herdr", ["session", "delete", session], {
          env: environment,
          stdio: "ignore",
          timeout: 15_000,
        });
      } catch {
        /* the session may already be gone */
      }
      rmSync(root, { recursive: true, force: true });
    }
  },
);

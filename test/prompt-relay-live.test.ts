import { createHash } from "node:crypto";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import { ControllerCore } from "../src/controller/core.js";
import { Launcher } from "../src/launcher.js";
import {
  assertNoNewDefaultWorkspaces,
  defaultSessionSnapshot,
  liveUnavailable,
  startLiveEnvironment,
} from "./herdr-live.js";
import { ctx, projectInfo } from "./harness.js";

/** The live harness plus a real Claude Code with a login in the operator's home, which the worker uses in place. */
function relayUnavailable(): string | undefined {
  const harness = liveUnavailable();
  if (harness !== undefined) return harness;
  try {
    execFileSync("claude", ["--version"], { stdio: "ignore" });
  } catch {
    return "claude is not available";
  }
  const home = process.env.HOME;
  if (home === undefined || !existsSync(path.join(home, ".claude")))
    return "no Claude Code login in the operator's home";
  return undefined;
}

const UNAVAILABLE = relayUnavailable();
if (UNAVAILABLE !== undefined)
  console.error(
    `SKIPPED LOUDLY: live prompt relay test did not run: ${UNAVAILABLE}`,
  );

const hashOf = (name: string): string =>
  createHash("sha256").update(name).digest("hex");

function configFor(): CapstanConfig {
  const role = (name: string, kind: "PM" | "Developer") => ({
    name,
    kind,
    host: "claude",
    model: null,
    permissionMode: "default" as const,
    allow: [] as string[],
    deny: [] as string[],
    hooks: "off" as const,
    prompt: { source: "none" as const, path: null, hash: null },
    configHash: hashOf(name),
  });
  const withText = (value: ReturnType<typeof role>) =>
    Object.defineProperty(value, "promptText", {
      value: null,
      enumerable: false,
    });
  return {
    schemaVersion: 1,
    projectName: null,
    herdrSession: "unused",
    notifications: { herdr: false, fallback: true },
    timers: {
      maxDeferralSeconds: 120,
      maxBusyDeferralSeconds: 120,
      pmAckTimeoutSeconds: 600,
      pmNotifyAfterSeconds: 300,
      notifyIntervalSeconds: 600,
      stallAfterSeconds: 900,
      workerAckTimeoutSeconds: 600,
      pmWakeAfterSeconds: 0,
      pmWakeIntervalSeconds: 120,
      findingCheckSeconds: 1800,
    },
    limits: { maxWorkers: 1 },
    layout: {
      spawn: "tab" as const,
      pmWidthPercent: 60,
      minPaneColumns: 40,
      minPaneRows: 12,
    },
    env: { pass: [] },
    hosts: [
      {
        name: "claude",
        kind: "claude",
        command: "claude",
        shellCommandTimeoutSeconds: 120,
        waitTimeoutSeconds: 45,
      },
    ],
    roles: [
      withText(role("pm", "PM")),
      withText(role("developer", "Developer")),
    ],
  } as unknown as CapstanConfig;
}

async function until<T>(
  what: string,
  check: () => Promise<T | undefined>,
  timeoutMs = 90_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

test(
  "a real Claude worker blocked on a permission prompt is captured and answered through the launcher; a stale hash types nothing",
  { skip: UNAVAILABLE, timeout: 420_000 },
  async () => {
    const before = defaultSessionSnapshot();
    // The scratch HOME's rc files set the bash prompt the launcher waits for, so the Herdr server must start bash panes whatever the operator's login shell is.
    const loginShell = process.env.SHELL;
    process.env.SHELL = "/bin/bash";
    const live = await startLiveEnvironment().finally(() => {
      if (loginShell === undefined) delete process.env.SHELL;
      else process.env.SHELL = loginShell;
    });
    const info = projectInfo();
    const stateDirectory = path.join(live.repo, ".capstan", "state");
    const core = await ControllerCore.open({ stateDirectory, project: info });
    const owner = info.ownerCredential;
    try {
      core.syncRoleDefinitions(ctx(core, owner), [
        { name: "pm", kind: "PM", host: "claude", configHash: hashOf("pm") },
        {
          name: "developer",
          kind: "Developer",
          host: "claude",
          configHash: hashOf("developer"),
        },
      ]);
      const cli = path.join(live.root, "watch-cli.js");
      writeFileSync(
        cli,
        "console.log('WATCH-PANE-RUNNING'); setInterval(() => {}, 1000);\n",
      );
      chmodSync(cli, 0o644);
      const events: Array<[string, Record<string, unknown>]> = [];
      // The harness puts a stand-in claude first on PATH under a scratch HOME; the worker gets the operator's HOME and PATH, so the real claude and its login are used in place.
      const baseEnvironment: NodeJS.ProcessEnv = {
        ...live.serverEnvironment,
        HOME: process.env.HOME!,
        PATH: process.env.PATH!,
      };
      const launcher = new Launcher({
        core,
        adapter: live.adapter,
        config: configFor(),
        projectRoot: live.repo,
        cliPath: cli,
        socketPath: path.join(stateDirectory, "control.sock"),
        credential: owner,
        baseEnvironment,
        log: (event, details) => events.push([event, details]),
      });
      const pm = await launcher.launchPm();
      await live.runner(["pane", "send-keys", pm.paneId!, "down"]);
      await live.runner(["pane", "send-keys", pm.paneId!, "enter"]);
      await until("the PM to reach its idle screen", async () =>
        (await live.adapter.agentObservation("pm-1")) === "idle"
          ? true
          : undefined,
      );
      const spawned = await launcher.spawn("developer");
      assert.equal(spawned.state, "started", JSON.stringify(spawned));
      const agent = spawned.agentId;
      const status = async (): Promise<string> =>
        live.adapter.agentObservation(agent);
      await until("the worker to be idle", async () =>
        (await status()) === "idle" ? true : undefined,
      );
      const ask = async (file: string): Promise<void> => {
        const result = await live.runner([
          "agent",
          "prompt",
          agent,
          `Run exactly this shell command and nothing else: touch ${file}`,
        ]);
        assert.equal(result.code, 0, result.stderr);
        await until("the worker to block on the permission prompt", async () =>
          (await status()) === "blocked" ? true : undefined,
        );
      };
      const keyEvents = (): number =>
        events.filter(([name]) => name === "prompt_relay_key").length;
      const typedInto = async (): Promise<string> =>
        live.adapter.readScreen(spawned.paneId!);

      // First prompt: capture, answer Yes with the captured hash, and the worker leaves the blocked state.
      await ask("relay-one.txt");
      const outcome = await launcher.capturePrompt(agent);
      assert.ok(outcome.captured, JSON.stringify(outcome));
      const prompt = outcome.prompt;
      assert.match(prompt.promptSha, /^[0-9a-f]{64}$/);
      assert.ok(prompt.text.includes("touch relay-one.txt"), prompt.text);
      assert.deepEqual(
        prompt.options.map((option) => option.number),
        prompt.options.map((_, index) => index + 1),
      );
      const yes = prompt.options.find((option) => option.text === "Yes");
      assert.ok(yes, JSON.stringify(prompt.options));
      let announced = 0;
      const answered = await launcher.answerPrompt(agent, {
        promptSha: prompt.promptSha,
        answer: { kind: "option", number: yes.number },
        beforeType: () => {
          announced += 1;
        },
      });
      assert.equal(answered.typed, true, JSON.stringify(answered));
      assert.equal(announced, 1);
      await until("the worker to leave the blocked state", async () =>
        (await status()) !== "blocked" ? true : undefined,
      );
      await until("the permitted command to have run", async () =>
        existsSync(path.join(spawned.worktreePath, "relay-one.txt"))
          ? true
          : undefined,
      );

      // The old hash no longer opens anything: first once the worker is not blocked, then against a different prompt.
      const keysAfterFirst = keyEvents();
      const idleStale = await launcher.answerPrompt(agent, {
        promptSha: prompt.promptSha,
        answer: { kind: "option", number: yes.number },
        beforeType: () => {
          announced += 1;
        },
      });
      assert.equal(idleStale.typed, false);
      assert.deepEqual(idleStale.keys, []);
      await until("the worker to settle", async () =>
        ["idle", "done"].includes(await status()) ? true : undefined,
      );
      await ask("relay-two.txt");
      const stale = await launcher.answerPrompt(agent, {
        promptSha: prompt.promptSha,
        answer: { kind: "option", number: yes.number },
        beforeType: () => {
          announced += 1;
        },
      });
      assert.deepEqual(stale, {
        typed: false,
        reason: "prompt_changed",
        keys: [],
      });
      assert.equal(announced, 1, "beforeType ran only for the first answer");
      assert.equal(keyEvents(), keysAfterFirst, "nothing was typed");
      assert.ok(
        (await typedInto()).includes("touch relay-two.txt"),
        "the second prompt is still open",
      );

      // The second prompt answered with text through the open field: the worker is told not to run it.
      const second = await launcher.capturePrompt(agent);
      assert.ok(second.captured, JSON.stringify(second));
      const no = second.prompt.options.find(
        (option) => option.text === "No" && option.acceptsText,
      );
      assert.ok(no, JSON.stringify(second.prompt.options));
      const texted = await launcher.answerPrompt(agent, {
        promptSha: second.prompt.promptSha,
        answer: { kind: "text", number: no.number, text: "do not touch files" },
        beforeType: () => undefined,
      });
      assert.equal(texted.typed, true, JSON.stringify(texted));
      await until("the worker to leave the blocked state again", async () =>
        (await status()) !== "blocked" ? true : undefined,
      );
      assert.equal(
        existsSync(path.join(spawned.worktreePath, "relay-two.txt")),
        false,
        "the refused command did not run",
      );
    } finally {
      core.close();
      await live.cleanup();
    }
    assertNoNewDefaultWorkspaces(before);
  },
);

import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";
import {
  ClearFailed,
  PmPaneError,
  buildAgentEnvironment,
  claudeArguments,
  type KeyLogEntry,
} from "../src/herdr/adapter.js";
import {
  defaultSessionSnapshot,
  liveUnavailable,
  startLiveEnvironment,
} from "./herdr-live.js";

const TOKEN = "tok_LIVE_0123456789ABCDEFGHIJKLMNOPQRSTUV";
const UNAVAILABLE = liveUnavailable();
if (UNAVAILABLE !== undefined)
  console.error(`SKIPPED LOUDLY: live Herdr tests did not run: ${UNAVAILABLE}`);

async function until<T>(
  action: () => Promise<T | undefined>,
  what: string,
  timeoutMs = 15_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await action();
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

test(
  "the adapter drives a real isolated Herdr session with a stand-in agent",
  { skip: UNAVAILABLE, timeout: 180_000 },
  async () => {
    const before = defaultSessionSnapshot();
    const live = await startLiveEnvironment();
    try {
      const { adapter, repo } = live;
      assert.equal(await adapter.version(), "herdr 0.9.1");

      const workspace = await adapter.createWorkspace({
        cwd: repo,
        label: "live-root",
        role: "PM",
      });
      const worktree = await adapter.createWorktree({
        workspaceId: workspace.workspaceId,
        branch: "cap/live/dev-g1",
        label: "live-dev",
      });
      assert.ok(existsSync(worktree.path));

      // A clean shell holds the token and the allowlist but nothing else.
      const environment = buildAgentEnvironment(
        { ...live.serverEnvironment, PLANTED_SECRET: "planted" },
        {
          CAPSTAN_TOKEN: TOKEN,
          CAPSTAN_SOCKET: "/tmp/none.sock",
          FAKE_CLAUDE_FIXTURES: live.serverEnvironment.FAKE_CLAUDE_FIXTURES!,
          FAKE_CLAUDE_ARGS: live.argsLog,
          FAKE_CLAUDE_LOG: live.messageLog,
        },
      );
      await adapter.prepareShell({ paneId: worktree.paneId, environment });
      const screen = await adapter.readScreen(worktree.paneId);
      assert.ok(!screen.includes(TOKEN), "the token is not on the screen");
      assert.equal(await adapter.readInput(worktree.paneId), "");
      await live.runner([
        "pane",
        "run",
        worktree.paneId,
        "echo planted=[${PLANTED_SECRET-unset}] token=[${CAPSTAN_TOKEN:0:8}] herdr=[${HERDR_ENV-unset}]",
      ]);
      const shown = await until(async () => {
        const text = await adapter.readScreen(worktree.paneId);
        return /^planted=.*$/m.exec(text)?.[0];
      }, "the environment report");
      assert.equal(shown, "planted=[unset] token=[tok_LIVE] herdr=[unset]");
      await until(
        async () =>
          (await adapter.readInput(worktree.paneId)) === "" ? true : undefined,
        "the prompt to return",
      );

      const roleArguments = claudeArguments({
        model: "fake-model",
        permissionMode: "acceptEdits",
        allow: ["Read"],
        deny: ["Bash(rm *)"],
        hooks: "off",
      });
      const started = await adapter.startAgent({
        name: "live-dev",
        kind: "claude",
        paneId: worktree.paneId,
        args: roleArguments,
        timeoutMs: 20_000,
      });
      assert.equal(started.status, "blocked_at_startup");

      const deferred = await adapter.guardedSend({
        paneId: worktree.paneId,
        text: "too early",
        beforeSend: () => assert.fail("nothing is recorded for a deferral"),
      });
      assert.deepEqual(deferred, { sent: false, reason: "agent_blocked" });

      const keys: string[] = [];
      const answered = await adapter.answerTrustDialog({
        paneId: worktree.paneId,
        log: (entry: KeyLogEntry) => {
          keys.push(entry.key);
        },
      });
      assert.deepEqual(answered, { handled: true, keys: ["down", "enter"] });
      assert.deepEqual(keys, ["down", "enter"]);

      await until(
        async () =>
          (await adapter.agentState("live-dev")).status === "idle"
            ? true
            : undefined,
        "the agent to become idle",
      );
      assert.equal(await adapter.readInput(worktree.paneId), "");

      let recorded = 0;
      const sent = await adapter.guardedSend({
        paneId: worktree.paneId,
        text: "first message",
        beforeSend: () => {
          recorded += 1;
        },
      });
      assert.deepEqual(sent, { sent: true });
      assert.equal(recorded, 1);
      await until(
        async () =>
          existsSync(live.messageLog) &&
          readFileSync(live.messageLog, "utf8").includes("first message")
            ? true
            : undefined,
        "the stand-in agent to log the message",
      );
      const argumentLines = readFileSync(live.argsLog, "utf8")
        .trimEnd()
        .split("\n");
      assert.deepEqual(argumentLines, roleArguments);

      // Text left in the input line defers a send and is cleared on request.
      await live.runner(["pane", "send-keys", worktree.paneId, "l"]);
      await live.runner(["pane", "send-keys", worktree.paneId, "e"]);
      await until(
        async () =>
          (await adapter.readInput(worktree.paneId)) === "le"
            ? true
            : undefined,
        "typed text to show",
      );
      const blocked = await adapter.guardedSend({
        paneId: worktree.paneId,
        text: "second",
        beforeSend: () => assert.fail("nothing is recorded for a deferral"),
      });
      assert.deepEqual(blocked, { sent: false, reason: "input_not_empty" });
      const discarded: string[] = [];
      const cleared = await adapter.clearAfterDeferral({
        paneId: worktree.paneId,
        deferredForMs: 1,
        maxDeferralMs: 1,
        discard: (text) => {
          discarded.push(text);
        },
        log: () => {},
      });
      assert.deepEqual(cleared, { cleared: true, text: "le" });
      assert.deepEqual(discarded, ["le"]);
      assert.equal(await adapter.readInput(worktree.paneId), "");
      void ClearFailed;

      // The PM pane is never typed into once its agent has started.
      await adapter.startAgent({
        name: "live-pm",
        kind: "claude",
        paneId: workspace.paneId,
        args: [],
        timeoutMs: 20_000,
      });
      const calls: string[][] = [];
      await assert.rejects(
        adapter.guardedSend({
          paneId: workspace.paneId,
          text: "never",
          beforeSend: () => {
            calls.push(["record"]);
          },
        }),
        PmPaneError,
      );
      assert.deepEqual(calls, []);
      await assert.rejects(
        adapter.answerTrustDialog({ paneId: workspace.paneId, log: () => {} }),
        PmPaneError,
      );

      await adapter.removeWorktree(worktree.workspaceId, { force: true });
      assert.equal(adapter.paneEntry(worktree.paneId), undefined);
    } finally {
      await live.cleanup();
    }
    assert.equal(
      defaultSessionSnapshot(),
      before,
      "the operator's default session is unchanged",
    );
  },
);

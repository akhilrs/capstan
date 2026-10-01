import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { chmodSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import { ControllerCore } from "../src/controller/core.js";
import { DeliveryDriver } from "../src/driver.js";
import { HerdrAdapter } from "../src/herdr/adapter.js";
import { Launcher, LauncherError } from "../src/launcher.js";
import type { Notifier } from "../src/notifier.js";
import {
  defaultSessionSnapshot,
  liveUnavailable,
  startLiveEnvironment,
} from "./herdr-live.js";
import { ctx, projectInfo } from "./harness.js";

const UNAVAILABLE = liveUnavailable();
if (UNAVAILABLE !== undefined)
  console.error(
    `SKIPPED LOUDLY: live launcher tests did not run: ${UNAVAILABLE}`,
  );

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
    configHash: name.padEnd(64, "0").slice(0, 64),
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
      pmAckTimeoutSeconds: 600,
      pmNotifyAfterSeconds: 300,
      notifyIntervalSeconds: 600,
      stallAfterSeconds: 900,
      workerAckTimeoutSeconds: 600,
    },
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

const silentNotifier: Notifier = {
  send: async () => [{ channel: "fallback", ok: true }],
  write: () => undefined,
};

async function until<T>(
  what: string,
  check: () => Promise<T | undefined>,
  timeoutMs = 20_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

/** Ticks the driver until the message is sent: a worker that is momentarily busy defers it, which is correct behavior. */
async function deliver(
  driver: DeliveryDriver,
  core: ControllerCore,
  messageId: string,
): Promise<void> {
  await until("the driver to deliver the message", async () => {
    await driver.tick();
    return core.message(messageId)!.state === "sent" ? true : undefined;
  });
}

test(
  "the launcher starts a PM and a worker in a real isolated Herdr session, delivers to the worker, re-adopts the panes after a restart and keeps the worker through a PM restart",
  { skip: UNAVAILABLE, timeout: 300_000 },
  async () => {
    const before = defaultSessionSnapshot();
    const live = await startLiveEnvironment();
    const info = projectInfo();
    const stateDirectory = path.join(live.repo, ".capstan", "state");
    const core = await ControllerCore.open({ stateDirectory, project: info });
    const owner = info.ownerCredential;
    try {
      core.syncRoleDefinitions(ctx(core, owner), [
        { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
        {
          name: "developer",
          kind: "Developer",
          host: "claude",
          configHash: "b".repeat(64),
        },
      ]);
      const cli = path.join(live.root, "watch-cli.js");
      writeFileSync(
        cli,
        "console.log('WATCH-PANE-RUNNING'); setInterval(() => {}, 1000);\n",
      );
      chmodSync(cli, 0o644);
      const events: string[] = [];
      const make = (adapter: HerdrAdapter) =>
        new Launcher({
          core,
          adapter,
          config: configFor(),
          projectRoot: live.repo,
          cliPath: cli,
          socketPath: path.join(stateDirectory, "control.sock"),
          credential: owner,
          baseEnvironment: live.serverEnvironment,
          log: (event, details) =>
            events.push(JSON.stringify([event, details])),
        });
      const head = execFileSync("git", ["rev-parse", "HEAD"], {
        cwd: live.repo,
        encoding: "utf8",
      }).trim();
      const screenOf = async (
        adapter: HerdrAdapter,
        paneId: string,
      ): Promise<string> => adapter.readScreen(paneId);
      const answerDialog = async (paneId: string): Promise<void> => {
        await live.runner(["pane", "send-keys", paneId, "down"]);
        await live.runner(["pane", "send-keys", paneId, "enter"]);
      };

      // Launch the PM: the fake stands at the real trust dialog, which only the operator answers.
      const first = new HerdrAdapter({ run: live.runner, tempRoot: live.root });
      const launcher = make(first);
      const launched = await launcher.launchPm();
      assert.equal(launched.state, "blocked", JSON.stringify(launched));
      assert.equal(launched.agentId, "pm-1");
      assert.equal(launched.hub, "opened");
      const pmPane = launched.paneId!;
      await answerDialog(pmPane);
      await until("the PM to reach its idle screen", async () =>
        (await first.agentObservation("pm-1")) === "idle" ? true : undefined,
      );
      const watch = core.fallbackPane(owner)!;
      await until("the watch pane", async () =>
        (await screenOf(first, watch.paneId)).includes("WATCH-PANE-RUNNING")
          ? true
          : undefined,
      );

      // Spawn a worker: a real worktree on its own branch at the recorded base sha, dialog answered by the controller.
      const spawned = await launcher.spawn("developer");
      assert.equal(spawned.state, "started", JSON.stringify(spawned));
      assert.equal(spawned.branch, "capstan/developer-1");
      assert.equal(
        execFileSync("git", ["rev-parse", "capstan/developer-1"], {
          cwd: live.repo,
          encoding: "utf8",
        }).trim(),
        head,
      );
      assert.match(
        execFileSync("git", ["worktree", "list", "--porcelain"], {
          cwd: live.repo,
          encoding: "utf8",
        }),
        /branch refs\/heads\/capstan\/developer-1/,
      );
      await until("the worker to be idle", async () =>
        (await first.agentObservation("developer-1")) === "idle"
          ? true
          : undefined,
      );
      await assert.rejects(
        launcher.spawn("developer"),
        (error: unknown) =>
          error instanceof LauncherError && error.code === "role_active",
      );

      // Deliver through the driver.
      const driver = new DeliveryDriver({
        core,
        adapter: first,
        timers: configFor().timers,
        notifier: silentNotifier,
        credential: owner,
      });
      const one = core.enqueueMessage(ctx(core, owner), {
        recipientAgentId: "developer-1",
        body: "build the first slice",
      }).messageId;
      await deliver(driver, core, one);
      const workerPane = first.paneForAgent("developer-1")!;
      await until("the worker to show the message", async () =>
        (await screenOf(first, workerPane)).includes(
          `[capstan message ${one} from operator]`.replace(/ /g, " "),
        ) ||
        (await screenOf(first, workerPane)).includes("build the first slice")
          ? true
          : undefined,
      );

      // A daemon restart: a second adapter re-registers the recorded panes and delivery continues.
      core.resolveMessage(ctx(core, owner), one, "skip");
      const second = new HerdrAdapter({
        run: live.runner,
        tempRoot: live.root,
      });
      const relaunched = make(second);
      await relaunched.adoptAll();
      assert.equal(second.paneForAgent("pm-1"), pmPane);
      assert.equal(second.paneForAgent("developer-1"), workerPane);
      const driver2 = new DeliveryDriver({
        core,
        adapter: second,
        timers: configFor().timers,
        notifier: silentNotifier,
        credential: owner,
      });
      const two = core.enqueueMessage(ctx(core, owner), {
        recipientAgentId: "developer-1",
        body: "second slice after the restart",
      }).messageId;
      await deliver(driver2, core, two);
      await until("the worker to show the second message", async () =>
        (await screenOf(second, workerPane)).includes(
          "second slice after the restart",
        )
          ? true
          : undefined,
      );
      assert.equal((await relaunched.launchPm()).state, "running");

      // A PM restart replaces the PM pane and its token; the worker pane stays.
      core.resolveMessage(ctx(core, owner), two, "skip");
      const restarted = await relaunched.restartPm();
      assert.equal(
        restarted.state,
        "blocked",
        JSON.stringify([restarted, relaunched.status(), events]),
      );
      assert.equal(restarted.generation, 2);
      assert.notEqual(restarted.paneId, pmPane);
      const rows = core.pmRestarts(owner, "pm-1");
      assert.equal(rows.length, 1);
      assert.equal(rows[0]!.consumed, true);
      const three = core.enqueueMessage(ctx(core, owner), {
        recipientAgentId: "developer-1",
        body: "third slice after the PM restart",
      }).messageId;
      await deliver(driver2, core, three);
      await until("the worker to show the third message", async () =>
        (await screenOf(second, workerPane)).includes(
          "third slice after the PM restart",
        )
          ? true
          : undefined,
      );
      const panes = await live.runner(["pane", "get", pmPane]);
      assert.notEqual(panes.code, 0, "the old PM pane is gone");
    } finally {
      core.close();
      await live.cleanup();
    }
    assert.equal(
      defaultSessionSnapshot(),
      before,
      "the operator's default session is unchanged",
    );
  },
);

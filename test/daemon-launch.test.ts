import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { callDaemon } from "../src/client.js";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import { ControllerCore } from "../src/controller/core.js";
import { runDaemon, type LogEntry } from "../src/daemon.js";
import type { Notifier } from "../src/notifier.js";
import { ctx, projectInfo } from "./harness.js";
import { StubAdapter } from "./launcher-stubs.js";

function configFor(): CapstanConfig {
  const role = Object.defineProperty(
    {
      name: "pm",
      kind: "PM",
      host: "claude",
      model: null,
      permissionMode: "default",
      allow: [],
      deny: [],
      hooks: "off",
      prompt: { source: "none", path: null, hash: null },
      configHash: "a".repeat(64),
    },
    "promptText",
    { value: null, enumerable: false },
  );
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
    roles: [role],
  } as unknown as CapstanConfig;
}

const notifier: Notifier = {
  send: async () => [{ channel: "fallback", ok: true }],
  write: () => undefined,
};

test("the daemon launches the PM through its socket and, after a restart, re-registers the recorded pane before the driver starts", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-daemon-launch-"));
  const stateDirectory = path.join(root, "state");
  const info = projectInfo();
  const seed = await ControllerCore.open({ stateDirectory, project: info });
  seed.syncRoleDefinitions(ctx(seed, info.ownerCredential), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
  ]);
  seed.close();
  const socket = path.join(stateDirectory, "control.sock");
  const operator = info.ownerCredential;

  const start = async (adapter: StubAdapter) => {
    const log: LogEntry[] = [];
    let ready!: () => void;
    const up = new Promise<void>((resolve) => {
      ready = resolve;
    });
    const done = runDaemon({
      stateDirectory,
      project: info,
      workspaceRoot: root,
      log: (entry) => log.push(entry),
      announce: (event) => {
        if (event.event === "ready") ready();
      },
      capstan: configFor(),
      adapter,
      notifier,
      cliPath: "/opt/capstan/cli.js",
      tickMs: 500,
    });
    await up;
    return { log, done };
  };
  const command = async (name: string, args: string[] = []) => {
    const result = await callDaemon(socket, operator, name, args, 30_000);
    assert.equal(result.kind, "response");
    return (
      result as {
        response: {
          ok: boolean;
          result?: unknown;
          code?: string;
          message?: string;
        };
      }
    ).response;
  };

  try {
    const first = new StubAdapter();
    const one = await start(first);
    const launched = await command("launch");
    assert.ok(launched.ok, JSON.stringify(launched));
    assert.equal((launched.result as { state: string }).state, "started");
    const status = await command("status");
    const panes = (
      status.result as { panes: Array<{ agentId: string; paneId: string }> }
    ).panes;
    assert.equal(panes.length, 1);
    assert.equal(panes[0]!.agentId, "pm-1");
    assert.ok(
      !JSON.stringify(status.result).includes(
        first.starts[0]!.environment!.CAPSTAN_TOKEN!,
      ),
    );
    await command("shutdown");
    await one.done;

    const second = new StubAdapter();
    assert.equal(second.paneForAgent("pm-1"), undefined);
    const two = await start(second);
    const deadline = Date.now() + 5000;
    while (second.paneForAgent("pm-1") === undefined && Date.now() < deadline)
      await new Promise((resolve) => setTimeout(resolve, 50));
    assert.equal(
      second.paneForAgent("pm-1"),
      panes[0]!.paneId,
      "the recorded pane was adopted",
    );
    assert.ok(second.calls.some((c) => c.startsWith("adopt:")));
    const again = await command("launch");
    assert.equal((again.result as { state: string }).state, "running");
    const restarted = await command("pm-restart");
    assert.ok(restarted.ok, JSON.stringify(restarted));
    assert.equal((restarted.result as { generation: number }).generation, 2);
    await command("shutdown");
    await two.done;
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

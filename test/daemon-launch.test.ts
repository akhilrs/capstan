import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
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
  const roleOf = (name: string, kind: string, hash: string) =>
    Object.defineProperty(
      {
        name,
        kind,
        host: "claude",
        model: null,
        permissionMode: "default",
        allow: [],
        deny: [],
        hooks: "off",
        prompt: { source: "none", path: null, hash: null },
        configHash: hash.repeat(64),
      },
      "promptText",
      { value: null, enumerable: false },
    );
  const roles = [
    roleOf("pm", "PM", "a"),
    roleOf("developer", "Developer", "b"),
  ];
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
      findingCheckSeconds: 1800,
    },
    limits: { maxWorkers: 3 },
    ledger: { keepMigrationBackups: 3 },
    layout: {
      spawn: "tab",
      split: "auto",
      minPaneColumns: 60,
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
    roles,
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

test("a daemon given syncRoles brings the roles into the ledger, so launch works on a fresh project without a separate config sync", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-daemon-sync-"));
  const stateDirectory = path.join(root, "state");
  const info = projectInfo();
  const socket = path.join(stateDirectory, "control.sock");
  let ready!: () => void;
  const up = new Promise<void>((resolve) => {
    ready = resolve;
  });
  const done = runDaemon({
    stateDirectory,
    project: info,
    workspaceRoot: root,
    log: () => undefined,
    announce: (event) => {
      if (event.event === "ready") ready();
    },
    capstan: configFor(),
    adapter: new StubAdapter(),
    notifier,
    cliPath: "/opt/capstan/cli.js",
    tickMs: 500,
    syncRoles: (core) => {
      core.syncRoleDefinitions(ctx(core, info.ownerCredential), [
        { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
      ]);
    },
  });
  try {
    await up;
    const launched = await callDaemon(
      socket,
      info.ownerCredential,
      "launch",
      [],
      30_000,
    );
    assert.equal(launched.kind, "response");
    const response = (
      launched as { response: { ok: boolean; result?: { state: string } } }
    ).response;
    assert.ok(response.ok, JSON.stringify(response));
    assert.equal(response.result!.state, "started");
    await callDaemon(socket, info.ownerCredential, "shutdown", [], 30_000);
    await done;
  } finally {
    await callDaemon(socket, info.ownerCredential, "shutdown", [], 5_000).catch(
      () => undefined,
    );
    await done.catch(() => undefined);
    rmSync(root, { recursive: true, force: true });
  }
});

test("a sync that fails at daemon start leaves the daemon up, and launch names the failure", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-daemon-syncfail-"));
  const stateDirectory = path.join(root, "state");
  const info = projectInfo();
  const socket = path.join(stateDirectory, "control.sock");
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
    adapter: new StubAdapter(),
    notifier,
    cliPath: "/opt/capstan/cli.js",
    tickMs: 500,
    syncRoles: () => {
      throw new Error("the ledger refused the change");
    },
  });
  try {
    await up;
    assert.ok(log.some((entry) => entry.command === "daemon:role_sync_failed"));
    const launched = await callDaemon(
      socket,
      info.ownerCredential,
      "launch",
      [],
      30_000,
    );
    assert.equal(launched.kind, "response");
    const response = (
      launched as { response: { ok: boolean; message?: string } }
    ).response;
    assert.equal(response.ok, false);
    assert.match(response.message ?? "", /role_not_synced/);
    assert.match(response.message ?? "", /the ledger refused the change/);
    await callDaemon(socket, info.ownerCredential, "shutdown", [], 30_000);
    await done;
  } finally {
    await callDaemon(socket, info.ownerCredential, "shutdown", [], 5_000).catch(
      () => undefined,
    );
    await done.catch(() => undefined);
    rmSync(root, { recursive: true, force: true });
  }
});

test("the PM's own token spawns and releases a worker through the socket, and a worker's token is refused both", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-daemon-delegate-"));
  for (const args of [
    ["init", "-q"],
    ["config", "user.email", "t@example.com"],
    ["config", "user.name", "t"],
    ["commit", "-q", "--allow-empty", "-m", "base"],
  ])
    execFileSync("git", ["-C", root, ...args]);
  const stateDirectory = path.join(root, "state");
  const info = projectInfo();
  const socket = path.join(stateDirectory, "control.sock");
  const adapter = new StubAdapter();
  let ready!: () => void;
  const up = new Promise<void>((resolve) => {
    ready = resolve;
  });
  const done = runDaemon({
    stateDirectory,
    project: info,
    workspaceRoot: root,
    log: () => undefined,
    announce: (event) => {
      if (event.event === "ready") ready();
    },
    capstan: configFor(),
    adapter,
    notifier,
    cliPath: "/opt/capstan/cli.js",
    tickMs: 500,
    syncRoles: (core) => {
      core.syncRoleDefinitions(
        ctx(core, info.ownerCredential),
        configFor().roles.map((role) => ({
          name: role.name,
          kind: role.kind,
          host: role.host,
          configHash: role.configHash,
        })),
      );
    },
  });
  const ask = async (credential: string, name: string, args: string[] = []) => {
    const result = await callDaemon(socket, credential, name, args, 60_000);
    assert.equal(result.kind, "response");
    return (
      result as {
        response: {
          ok: boolean;
          code?: string;
          message?: string;
          result?: Record<string, unknown>;
        };
      }
    ).response;
  };
  try {
    await up;
    assert.ok((await ask(info.ownerCredential, "launch")).ok);
    const pmToken = adapter.starts[0]!.environment!.CAPSTAN_TOKEN!;
    const spawned = await ask(pmToken, "spawn", ["developer"]);
    assert.ok(spawned.ok, JSON.stringify(spawned));
    assert.equal(spawned.result!.agentId, "developer-1");
    const workerToken = adapter.starts[1]!.environment!.CAPSTAN_TOKEN!;
    for (const [command, args] of [
      ["spawn", ["developer"]],
      ["release", ["developer-1"]],
    ] as const) {
      const refused = await ask(workerToken, command, [...args]);
      assert.equal(refused.ok, false);
      assert.equal(refused.code, "forbidden", command);
    }
    const released = await ask(pmToken, "release", ["developer-1"]);
    assert.ok(released.ok, JSON.stringify(released));
    assert.equal(released.result!.state, "released");
    const again = await ask(pmToken, "release", ["developer-1"]);
    assert.equal(again.ok, false);
    assert.match(again.message ?? "", /agent_not_active/);
    await ask(info.ownerCredential, "shutdown");
    await done;
  } finally {
    await callDaemon(socket, info.ownerCredential, "shutdown", [], 5_000).catch(
      () => undefined,
    );
    await done.catch(() => undefined);
    rmSync(root, { recursive: true, force: true });
  }
});

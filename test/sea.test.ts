import assert from "node:assert/strict";

// No test may reach a real Herdr session: the daemon stays out of it.
process.env.CAPSTAN_LAUNCH = "off";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { afterEach, test } from "node:test";
import { callDaemon } from "../src/client.js";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import { readMigration } from "../src/controller/database.js";
import { runDaemon } from "../src/daemon.js";
import { cstanWrapperScript, selfInvocation } from "../src/launcher.js";
import type { Notifier } from "../src/notifier.js";
import { entryPath, isSea, selfCommand, setSeaForTests } from "../src/sea.js";
import { harness } from "./harness.js";
import { StubAdapter } from "./launcher-stubs.js";
import { operatorConfig } from "./operator-harness.js";
import { fakeProject, writeFakeBuild } from "./restart-fakes.js";

afterEach(() => setSeaForTests(undefined));

test("outside the binary selfCommand runs node, its flags, the CLI file and the arguments", () => {
  assert.equal(isSea(), false);
  const command = selfCommand(["daemon"], "/opt/capstan/cli.js");
  assert.equal(command.command, process.execPath);
  assert.deepEqual(command.args, [
    ...process.execArgv,
    "/opt/capstan/cli.js",
    "daemon",
  ]);
  assert.equal(path.basename(entryPath()), "cli.js");
});

test("under SEA selfCommand and entryPath name only the binary", () => {
  setSeaForTests(true);
  assert.equal(entryPath(), process.execPath);
  assert.deepEqual(selfCommand(["daemon"], "/opt/capstan/cli.js"), {
    command: process.execPath,
    args: ["daemon"],
  });
});

test("under SEA the agent wrapper and the status pane invoke only the binary", () => {
  setSeaForTests(true);
  const wrapper = cstanWrapperScript("/usr/bin/node", "/opt/capstan/cli.js");
  assert.equal(wrapper, `#!/bin/sh\nexec '${process.execPath}' "$@"\n`);
  assert.ok(!wrapper.includes("/usr/bin/node"));
  assert.ok(!wrapper.includes("cli.js"));
  const pane = `exec ${selfInvocation("/usr/bin/node", "/opt/capstan/cli.js")} status --watch`;
  assert.equal(pane, `exec '${process.execPath}' status --watch`);
});

test("outside SEA the wrapper keeps node and the CLI file", () => {
  assert.equal(
    cstanWrapperScript("/usr/bin/node", "/opt/capstan/cli.js"),
    `#!/bin/sh\nexec '/usr/bin/node' '/opt/capstan/cli.js' "$@"\n`,
  );
});

test("outside a real binary migrations come from disk even when SEA is mocked on", () => {
  setSeaForTests(true);
  assert.ok(readMigration("0001_initial.sql").length > 0);
});

const notifier: Notifier = {
  send: async () => [{ channel: "fallback", ok: true }],
  write: () => undefined,
};

function operatorOnConfig(): CapstanConfig {
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
    roles: [roleOf("pm", "PM", "a"), roleOf("developer", "Developer", "b")],
    operator: operatorConfig({ enabled: true }),
    architect: { enabled: false, role: "architect" },
  } as unknown as CapstanConfig;
}

test("under SEA the daemon runs with the Operator on and snapshots the binary file as known-good", async () => {
  const h = await harness();
  const project = fakeProject();
  writeFakeBuild(project.distDir, "good");
  const fakeBinary = path.join(project.root, "cstan-binary");
  writeFileSync(fakeBinary, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
  setSeaForTests(true);
  await h.server.close();
  h.core.close();
  let ready!: () => void;
  const up = new Promise<void>((resolve) => {
    ready = resolve;
  });
  const log: string[] = [];
  const done = runDaemon({
    stateDirectory: h.stateDirectory,
    project: h.info,
    workspaceRoot: project.root,
    log: (entry) => log.push(entry.command),
    announce: (event) => {
      if (event.event === "ready") ready();
    },
    capstan: operatorOnConfig(),
    adapter: new StubAdapter(),
    notifier,
    cliPath: process.execPath,
    tickMs: 500,
    restart: { distDir: fakeBinary, knownGoodSettleMs: 100 },
  });
  await up;
  const socket = path.join(h.stateDirectory, "control.sock");
  try {
    const op = await callDaemon(socket, h.owner, "op", ["show"]);
    assert.doesNotMatch(JSON.stringify(op), /not_configured/);
    await new Promise((resolve) => setTimeout(resolve, 700));
    assert.deepEqual(
      readdirSync(path.join(h.stateDirectory, "known-good")).sort(),
      ["cstan", "manifest.json"],
    );
    assert.ok(log.some((entry) => /known_good_saved/.test(entry)));
    assert.equal(
      readFileSync(path.join(h.stateDirectory, "known-good", "cstan"), "utf8"),
      "#!/bin/sh\nexit 0\n",
    );
  } finally {
    await callDaemon(socket, h.owner, "shutdown", [], 5000).catch(
      () => undefined,
    );
    await done.catch(() => undefined);
    await project.cleanup();
  }
});

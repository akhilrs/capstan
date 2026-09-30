import assert from "node:assert/strict";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { spawnSync } from "node:child_process";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import { ControllerCore } from "../src/controller/core.js";
import { Launcher, LauncherError } from "../src/launcher.js";
import { CSTAN_ALLOW_RULE } from "../src/prompts.js";
import {
  AgentPaneMismatch,
  PaneGone,
  PromptUnrecognized,
  buildAgentEnvironment,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import { ctx, projectInfo } from "./harness.js";
import { SHA, StubAdapter, StubGit } from "./launcher-stubs.js";

function config(fallback = true): CapstanConfig {
  const role = (name: string, kind: "PM" | "Developer", extra = {}) => ({
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
    ...extra,
  });
  const withText = (value: ReturnType<typeof role>, text: string | null) =>
    Object.defineProperty(value, "promptText", {
      value: text,
      enumerable: false,
    });
  return {
    schemaVersion: 1,
    projectName: null,
    herdrSession: "test",
    notifications: { herdr: true, fallback },
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
      withText(role("pm", "PM"), "Keep the plan small."),
      withText(role("developer", "Developer"), null),
      withText(role("developer2", "Developer"), null),
    ],
  } as unknown as CapstanConfig;
}

interface World {
  core: ControllerCore;
  owner: string;
  adapter: StubAdapter;
  git: StubGit;
  launcher: Launcher;
  root: string;
  events: Array<{ event: string; details: Record<string, unknown> }>;
  reopen(): Launcher;
  cleanup(): void;
}

async function world(fallback = true): Promise<World> {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-launcher-"));
  const stateDirectory = path.join(root, ".capstan", "state");
  const info = projectInfo();
  const core = await ControllerCore.open({ stateDirectory, project: info });
  const owner = info.ownerCredential;
  core.syncRoleDefinitions(
    ctx(core, owner),
    ["pm:PM", "pm2:PM", "developer:Developer", "developer2:Developer"].map(
      (entry, index) => {
        const [name, kind] = entry.split(":") as [string, "PM" | "Developer"];
        return {
          name,
          kind,
          host: "claude",
          configHash: String(index).repeat(64),
        };
      },
    ),
  );
  const adapter = new StubAdapter();
  const git = new StubGit();
  const events: World["events"] = [];
  const make = (): Launcher =>
    new Launcher({
      core,
      adapter,
      config: config(fallback),
      projectRoot: root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(stateDirectory, "control.sock"),
      credential: owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: {
        PATH: "/usr/bin:/bin",
        HOME: "/home/x",
        LANG: "C",
        SECRET: "no",
      },
      git,
      log: (event, details) => events.push({ event, details }),
    });
  return {
    core,
    owner,
    adapter,
    git,
    launcher: make(),
    root,
    events,
    reopen: make,
    cleanup: () => {
      core.close();
      rmSync(root, { recursive: true, force: true });
      rmSync(adapter.dir, { recursive: true, force: true });
    },
  };
}

const eventNames = (w: World): string[] => w.events.map((e) => e.event);

test("launch creates the PM agent, starts it with its token, prompt and a cstan wrapper, records the pane and opens the watch pane", async () => {
  const w = await world();
  try {
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "started");
    assert.equal(result.agentId, "pm-1");
    assert.equal(result.hub, "opened");
    const agent = w.core.listAgents().find((a) => a.agentId === "pm-1")!;
    assert.equal(agent.kind, "PM");
    assert.equal(agent.state, "active");
    const start = w.adapter.starts[0]!;
    assert.equal(start.name, "pm-1");
    const environment = start.environment!;
    assert.match(environment.CAPSTAN_TOKEN!, /\S{20,}/);
    assert.equal(
      environment.CAPSTAN_SOCKET,
      path.join(w.root, ".capstan", "state", "control.sock"),
    );
    assert.equal(environment.PATH, `${w.root}/.capstan/bin:/usr/bin:/bin`);
    assert.equal(environment.SECRET, undefined, "only the allowlist passes");
    assert.ok(start.args.includes(CSTAN_ALLOW_RULE));
    const promptFile =
      start.args[start.args.indexOf("--append-system-prompt-file") + 1]!;
    const prompt = readFileSync(promptFile, "utf8");
    assert.match(prompt, /cstan inbox/);
    assert.match(prompt, /45 seconds/);
    assert.match(prompt, /Keep the plan small\./);
    assert.ok(
      !prompt.includes(environment.CAPSTAN_TOKEN!),
      "the token is never in the prompt",
    );
    const rows = w.core.agentPanes(w.owner);
    assert.equal(rows.length, 1);
    assert.equal(rows[0]!.agentId, "pm-1");
    assert.equal(rows[0]!.paneId, result.paneId);
    const fallback = w.core.fallbackPane(w.owner)!;
    assert.ok(
      w.adapter.calls.some(
        (c) =>
          c.startsWith(`run:${fallback.paneId}:`) &&
          c.includes("status --watch"),
      ),
    );
    assert.equal(
      w.adapter.lastShellEnvironment!.CAPSTAN_TOKEN,
      undefined,
      "the watch pane holds no token",
    );
    assert.equal(w.adapter.lastShellEnvironment!.CAPSTAN_SOCKET, undefined);
  } finally {
    w.cleanup();
  }
});

test("the cstan wrapper is a private script that runs the CLI with the recorded node, and is replaced atomically", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const wrapper = path.join(w.root, ".capstan", "bin", "cstan");
    assert.equal(statSync(wrapper).mode & 0o777, 0o700);
    assert.equal(statSync(path.dirname(wrapper)).mode & 0o777, 0o700);
    const text = readFileSync(wrapper, "utf8");
    assert.equal(
      text,
      `#!/bin/sh\nexec '/usr/bin/node' '/opt/capstan/cli.js' "$@"\n`,
    );

    const fake = path.join(w.root, "fake-node");
    const cli = path.join(w.root, "cli.js");
    writeFileSync(fake, '#!/bin/sh\necho "ran $1 $2"\n', { mode: 0o755 });
    chmodSync(fake, 0o755);
    writeFileSync(cli, "");
    writeFileSync(wrapper, `#!/bin/sh\nexec '${fake}' '${cli}' "$@"\n`, {
      mode: 0o700,
    });
    const run = spawnSync(wrapper, ["status"], { encoding: "utf8" });
    assert.equal(run.status, 0);
    assert.match(run.stdout, /ran .*cli.js status/);
  } finally {
    w.cleanup();
  }
});

test("a second launch reports the running PM and opens nothing new; a blocked PM is reported; a PM without a pane needs a restart", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const calls = w.adapter.calls.length;
    const again = await w.launcher.launchPm();
    assert.equal(again.state, "running");
    assert.equal(again.hub, "present");
    assert.equal(
      w.adapter.calls.length,
      calls,
      "no workspace, start or pane was created",
    );
    w.adapter.observation = "blocked";
    assert.equal((await w.launcher.launchPm()).state, "blocked");

    w.adapter.agentPanes.clear();
    const lost = await w.launcher.launchPm();
    assert.equal(lost.state, "needs_restart");
    assert.match(lost.hint!, /pm restart/);
  } finally {
    w.cleanup();
  }
});

test("a PM that is blocked at startup is reported with the hint and keeps its pane row; the hub opens even with the fallback channel off", async () => {
  const w = await world(false);
  try {
    w.adapter.startStatus = "blocked_at_startup";
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "blocked");
    assert.match(result.hint!, /trust dialog/);
    assert.equal(
      result.hub,
      "opened",
      "the hub opens whatever the notification channels say",
    );
    assert.equal(w.core.agentPanes(w.owner).length, 1);
    assert.ok(
      !w.adapter.calls.some((c) => c.startsWith("dialog:")),
      "the PM's dialog is never answered",
    );
  } finally {
    w.cleanup();
  }
});

test("a failed first start ends the new agent, closes its pane and leaves nothing behind, and a later launch starts fresh", async () => {
  const w = await world();
  try {
    w.adapter.startError = new PromptUnrecognized("the shell is not ready");
    const failed = await w.launcher.launchPm();
    assert.equal(failed.state, "failed");
    assert.match(failed.reason!, /shell is not ready/);
    assert.ok(w.adapter.calls.some((c) => c.startsWith("close:")));
    assert.equal(w.core.agentPanes(w.owner).length, 0);
    assert.equal(
      w.core.listAgents().filter((a) => a.state === "active").length,
      0,
    );

    w.adapter.startError = undefined;
    const fresh = await w.launcher.launchPm();
    assert.equal(fresh.state, "started");
    assert.equal(
      fresh.agentId,
      "pm-2",
      "a new incarnation gets a new id on the same seat",
    );
  } finally {
    w.cleanup();
  }
});

test("a crash between registering the PM and recording its pane leaves an active PM with no row: launch says needs_restart", async () => {
  const w = await world();
  try {
    const seat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "pm-seat",
      name: "pm",
      role: "PM",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "pm-1",
      role: "PM",
      seatId: seat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "pm-1",
      roleName: "pm",
      seatId: seat.seatId,
      actorId: actor.actorId,
    });
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "needs_restart");
  } finally {
    w.cleanup();
  }
});

test("two active PMs make launch refuse with pm_exists", async () => {
  const w = await world();
  try {
    for (const [name, seatName] of [
      ["pm", "pm"],
      ["pm2", "pm2"],
    ] as const) {
      const seat = w.core.createSeat(ctx(w.core, w.owner), {
        seatId: `${seatName}-seat`,
        name,
        role: "PM",
      });
      const actor = w.core.createActor(ctx(w.core, w.owner), {
        displayName: name,
        role: "PM",
        seatId: seat.seatId,
      });
      w.core.registerAgent(ctx(w.core, w.owner), {
        agentId: `${name}-1`,
        roleName: name,
        seatId: seat.seatId,
        actorId: actor.actorId,
      });
    }
    await assert.rejects(
      w.launcher.launchPm(),
      (e: unknown) => e instanceof LauncherError && e.code === "pm_exists",
    );
    await assert.rejects(
      w.launcher.restartPm(),
      (e: unknown) => e instanceof LauncherError && e.code === "pm_ambiguous",
    );
  } finally {
    w.cleanup();
  }
});

async function launched(w: World): Promise<void> {
  assert.equal((await w.launcher.launchPm()).state, "started");
}

test("spawn starts one worker in its own worktree with its token, prompt, worker profile and a base sha, and records the row", async () => {
  const w = await world();
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.equal(result.agentId, "developer-1");
    assert.equal(result.branch, "capstan/developer-1");
    assert.ok(w.adapter.calls.includes(`worktree:capstan/developer-1:${SHA}`));
    assert.deepEqual(
      w.adapter.worktreeParents,
      [w.core.fallbackPane(w.owner)!.workspaceId],
      "worktrees hang under the hub workspace, never under the PM's",
    );
    const start = w.adapter.starts.find((s) => s.name === "developer-1")!;
    assert.ok(start.args.includes(CSTAN_ALLOW_RULE));
    assert.ok(start.args.includes("--settings"), "hooks are off for a worker");
    assert.match(start.environment!.CAPSTAN_TOKEN!, /\S{20,}/);
    assert.equal(
      start.environment!.PATH!.startsWith(`${w.root}/.capstan/bin:`),
      true,
    );
    const prompt = readFileSync(
      start.args[start.args.indexOf("--append-system-prompt-file") + 1]!,
      "utf8",
    );
    assert.match(prompt, /cstan ack <message-id>/);
    assert.match(prompt, /cstan send @pm/);
    const row = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.deepEqual(
      [row.worktreePath, row.branch, row.baseSha, row.paneId === result.paneId],
      ["/tmp/work/developer-1", "capstan/developer-1", SHA, true],
    );
  } finally {
    w.cleanup();
  }
});

test("spawn refuses a second worker for an active role, an unknown role, a PM role and a missing PM, without side effects", async () => {
  const w = await world();
  try {
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "pm_not_launched",
    );
    await launched(w);
    await w.launcher.spawn("developer");
    const calls = w.adapter.calls.length;
    const agents = w.core.listAgents().length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) => e instanceof LauncherError && e.code === "role_active",
    );
    await assert.rejects(
      w.launcher.spawn("nobody"),
      (e: unknown) => e instanceof LauncherError && e.code === "unknown_role",
    );
    await assert.rejects(
      w.launcher.spawn("pm"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "kind_not_spawnable",
    );
    assert.equal(w.adapter.calls.length, calls);
    assert.equal(w.core.listAgents().length, agents);
    const other = await w.launcher.spawn("developer2");
    assert.equal(other.agentId, "developer2-1", "another role is independent");
  } finally {
    w.cleanup();
  }
});

test("a worker blocked at startup has its trust dialog answered once with every key logged; a refused answer is reported", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startStatus = "blocked_at_startup";
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.equal(
      w.adapter.calls.filter((c) => c.startsWith("dialog:")).length,
      1,
    );
    assert.deepEqual(
      w.events
        .filter((e) => e.event === "trust_dialog_key")
        .map((e) => e.details.key),
      ["down", "enter"],
    );
    w.adapter.dialogHandled = false;
    const refused = await w.launcher.spawn("developer2");
    assert.equal(refused.state, "blocked");
    assert.match(refused.hint!, /path_mismatch/);
  } finally {
    w.cleanup();
  }
});

test("a failed spawn ends the agent, closes the pane, removes the worktree, deletes the branch at the base sha and clears the row; a retry works", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new HerdrError("agent_start_failed", "no binary");
    await assert.rejects(w.launcher.spawn("developer"), HerdrError);
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer-1")!.state,
      "ended",
    );
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1", SHA]]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
    const order = w.adapter.calls.filter(
      (c) => c.startsWith("close:") || c.startsWith("worktree:"),
    );
    assert.ok(order.length >= 2);

    w.adapter.startError = undefined;
    const retry = await w.launcher.spawn("developer");
    assert.equal(retry.agentId, "developer-2");
    assert.equal(retry.branch, "capstan/developer-2");
  } finally {
    w.cleanup();
  }
});

test("cleanup never forces: a worktree git refuses to remove keeps its branch and is reported; a kept branch is logged", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    w.git.removeOk = false;
    await assert.rejects(w.launcher.spawn("developer"));
    assert.deepEqual(
      w.git.deleted,
      [],
      "the branch is not touched while the worktree stays",
    );
    assert.deepEqual(w.launcher.status().cleanupFailed, [
      {
        agentId: "developer-1",
        reason: "the worktree could not be removed without force",
        worktreePath: "/tmp/work/developer-1",
      },
    ]);
    w.git.removeOk = true;
    w.git.deleteOk = false;
    w.adapter.startError = new Error("again");
    await assert.rejects(w.launcher.spawn("developer"));
    assert.ok(eventNames(w).includes("branch_kept"));
  } finally {
    w.cleanup();
  }
});

test("a cleanup that cannot end the agent leaves it active, touches nothing else, and role_active names the reason", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    (w.core as unknown as { endAgent: () => never }).endAgent = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(w.launcher.spawn("developer"));
    assert.deepEqual(w.git.removed, []);
    assert.deepEqual(w.git.deleted, []);
    assert.equal(
      w.launcher.status().cleanupFailed[0]!.reason,
      "the seat still holds authority",
    );
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "role_active" &&
        /still holds authority/.test(e.message),
    );
  } finally {
    w.cleanup();
  }
});

test("a seat of another kind or a disabled seat is refused before anything is created", async () => {
  const w = await world();
  try {
    await launched(w);
    w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer2-seat",
      name: "developer2",
      role: "Verifier",
    });
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "seat_kind_mismatch" &&
        /Verifier/.test(e.message),
    );
    assert.equal(
      w.core.listAgents().some((a) => a.roleName === "developer2"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("an orphan actor left by a crash on a reused seat is revoked before a new agent is created", async () => {
  const w = await world();
  try {
    await launched(w);
    w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const orphan = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "orphan",
      role: "Developer",
      seatId: "developer-seat",
    });
    assert.deepEqual(w.core.seatActorIds(w.owner, "developer-seat"), [
      orphan.actorId,
    ]);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.ok(
      !w.core.seatActorIds(w.owner, "developer-seat").includes(orphan.actorId),
    );
  } finally {
    w.cleanup();
  }
});

test("operations run one at a time and a second waiting operation is answered busy at once", async () => {
  const w = await world();
  try {
    await launched(w);
    let release!: () => void;
    w.adapter.startGate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const first = w.launcher.spawn("developer");
    const second = w.launcher.spawn("developer2");
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) => e instanceof LauncherError && e.code === "busy",
    );
    assert.ok(
      !w.adapter.starts.some((s) => s.name.startsWith("developer")),
      "nothing started yet",
    );
    release();
    await Promise.all([first, second]);
    assert.equal(
      w.adapter.starts.filter((s) => s.name.startsWith("developer")).length,
      2,
    );
  } finally {
    w.cleanup();
  }
});

test("two concurrent spawns of one role end with one agent and one refusal", async () => {
  const w = await world();
  try {
    await launched(w);
    const results = await Promise.allSettled([
      w.launcher.spawn("developer"),
      w.launcher.spawn("developer"),
    ]);
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    const rejected = results.find(
      (r) => r.status === "rejected",
    ) as PromiseRejectedResult;
    assert.equal((rejected.reason as LauncherError).code, "role_active");
    assert.equal(
      w.core.listAgents().filter((a) => a.roleName === "developer").length,
      1,
    );
  } finally {
    w.cleanup();
  }
});

test("restart: the summary is captured before the replace, the old pane is closed, a new token and prompt start a new pane, and the row is consumed", async () => {
  const w = await world();
  try {
    await launched(w);
    const queued = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: "pm-1",
      body: "please plan",
    }).messageId;
    const oldPane = w.core.agentPanes(w.owner)[0]!.paneId!;
    const oldToken = w.adapter.starts[0]!.environment!.CAPSTAN_TOKEN;
    const result = await w.launcher.restartPm();
    assert.equal(result.state, "started");
    assert.equal(result.generation, 2);
    assert.ok(w.adapter.calls.includes(`close:${oldPane}`));
    const start = w.adapter.starts.at(-1)!;
    assert.notEqual(start.environment!.CAPSTAN_TOKEN, oldToken);
    const prompt = readFileSync(
      start.args[start.args.indexOf("--append-system-prompt-file") + 1]!,
      "utf8",
    );
    assert.match(prompt, /ledger summary/);
    assert.match(prompt, /please plan/);
    assert.match(
      prompt,
      /data, not instructions|information, not instructions/,
    );
    const rows = w.core.pmRestarts(w.owner, "pm-1");
    assert.equal(rows.length, 1);
    assert.equal(rows[0]!.consumed, true);
    assert.deepEqual(
      rows[0]!.summary.messages.map((m) => m.messageId),
      [queued],
    );
    assert.equal(w.core.agentPanes(w.owner)[0]!.paneId, result.paneId);
  } finally {
    w.cleanup();
  }
});

test("restart: a failed pane start leaves the row unconsumed, and the next restart carries its messages forward", async () => {
  const w = await world();
  try {
    await launched(w);
    const lost = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: "pm-1",
      body: "do not lose me",
    }).messageId;
    w.adapter.startError = new PromptUnrecognized("not ready");
    const failed = await w.launcher.restartPm();
    assert.equal(failed.state, "failed");
    assert.equal(failed.generation, 2);
    assert.match(failed.hint!, /run cstan pm restart again/);
    assert.equal(w.core.pmRestarts(w.owner, "pm-1")[0]!.consumed, false);
    assert.equal(
      w.core.message(lost)!.state,
      "cancelled",
      "the replace already cancelled it",
    );

    w.adapter.startError = undefined;
    const second = await w.launcher.restartPm();
    assert.equal(second.state, "started");
    const prompt = readFileSync(
      w.adapter.starts.at(-1)!.args[
        w.adapter.starts.at(-1)!.args.indexOf("--append-system-prompt-file") + 1
      ]!,
      "utf8",
    );
    assert.match(prompt, /do not lose me/);
    assert.ok(w.core.pmRestarts(w.owner, "pm-1").every((r) => r.consumed));
  } finally {
    w.cleanup();
  }
});

test("restart: a replace the core refuses leaves the old pane and the ledger untouched, and a missing PM is refused", async () => {
  const w = await world();
  try {
    await assert.rejects(
      w.launcher.restartPm(),
      (e: unknown) => e instanceof LauncherError && e.code === "no_pm",
    );
    await launched(w);
    const before = w.adapter.calls.length;
    (
      w.core as unknown as { restartAgentGeneration: () => never }
    ).restartAgentGeneration = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(w.launcher.restartPm(), /still holds authority/);
    assert.equal(
      w.adapter.calls.length,
      before,
      "no pane was closed or started",
    );
    assert.equal(w.core.agentPanes(w.owner).length, 1);
  } finally {
    w.cleanup();
  }
});

test("restart: an old pane that cannot be closed is listed as an orphan and the new PM still starts", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const result = await w.launcher.restartPm();
    assert.equal(result.state, "started");
    assert.equal(w.launcher.status().orphanPanes.length, 1);
    assert.equal(w.launcher.status().orphanPanes[0]!.agentId, "pm-1");
  } finally {
    w.cleanup();
  }
});

test("after a daemon restart the recorded panes are re-registered, lost ones are cleared, a lost worker is ended and a lost PM needs a restart", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer2");
    const fresh = new StubAdapter();
    const second = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: "/tmp/x.sock",
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: { PATH: "/usr/bin" },
      git: w.git,
      log: (event, details) => w.events.push({ event, details }),
    });
    const rows = w.core.agentPanes(w.owner);
    const lostPane = rows.find((r) => r.agentId === "developer2-1")!.paneId!;
    fresh.adoptErrors.set(lostPane, new PaneGone("gone"));
    await second.adoptAll();
    assert.equal(fresh.paneForAgent("pm-1") !== undefined, true);
    assert.equal(fresh.paneForAgent("developer-1") !== undefined, true);
    assert.ok(fresh.calls.some((c) => c.startsWith("adopt-shell:")));
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer2-1")!.state,
      "ended",
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer2-1"),
      false,
    );
    assert.ok(eventNames(w).includes("pane_lost"));
    assert.equal(
      (await second.spawn("developer2")).agentId,
      "developer2-2",
      "the role is free again",
    );

    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const third = new StubAdapter();
    third.adoptErrors.set(pmPane, new AgentPaneMismatch("elsewhere"));
    const launcher = new Launcher({
      core: w.core,
      adapter: third,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const result = await launcher.launchPm();
    assert.equal(result.state, "needs_restart");
  } finally {
    w.cleanup();
  }
});

test("adoption sweeps stale rows, crashed spawns with and without a row, and a vanished fallback pane", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    w.core.endAgent(ctx(w.core, w.owner), "developer-1");
    const crashedSeat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer2-seat",
      name: "developer2",
      role: "Developer",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "d2",
      role: "Developer",
      seatId: crashedSeat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "developer2-1",
      roleName: "developer2",
      seatId: crashedSeat.seatId,
      actorId: actor.actorId,
    });
    const fresh = new StubAdapter();
    const second = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
      log: (event, details) => w.events.push({ event, details }),
    });
    const fallback = w.core.fallbackPane(w.owner)!;
    fresh.adoptErrors.set(fallback.paneId, new PaneGone("gone"));
    await second.adoptAll();
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
      "the stale row is cleared",
    );
    assert.ok(
      fresh.calls.some((c) => c.startsWith("close:")),
      "the stale pane is closed",
    );
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer2-1")!.state,
      "ended",
      "a crashed spawn with no row is ended",
    );
    assert.equal(w.core.fallbackPane(w.owner), undefined);
    assert.ok(eventNames(w).includes("crashed_spawn"));
  } finally {
    w.cleanup();
  }
});

test("a crashed spawn with an intent row removes its worktree found by branch and deletes the branch at the recorded sha", async () => {
  const w = await world();
  try {
    await launched(w);
    const seat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "d",
      role: "Developer",
      seatId: seat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "developer-1",
      roleName: "developer",
      seatId: seat.seatId,
      actorId: actor.actorId,
    });
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "developer-1",
      workspaceId: null,
      paneId: null,
      worktreePath: null,
      branch: "capstan/developer-1",
      baseSha: SHA,
    });
    w.git.byBranch.set("capstan/developer-1", "/tmp/found/by/branch");
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/found/by/branch"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1", SHA]]);
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer-1")!.state,
      "ended",
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("a hub that fails to open is reported as failed, the PM still starts, and spawn refuses with hub_unavailable", async () => {
  const w = await world();
  try {
    w.adapter.runError = new Error("cannot run");
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "started");
    assert.equal(result.hub, "failed");
    assert.equal(w.core.fallbackPane(w.owner), undefined);
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "hub_unavailable",
    );
    assert.equal(
      w.core.listAgents().some((a) => a.roleName === "developer"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("the agent environment helper and the wrapper directory exist for every agent and the project files stay private", async () => {
  const w = await world();
  try {
    await launched(w);
    assert.ok(existsSync(path.join(w.root, ".capstan", "bin", "cstan")));
    mkdirSync(path.join(w.root, "x"), { recursive: true });
    const environment = buildAgentEnvironment(
      { PATH: "/p" },
      { PATH: "/a:/p" },
    );
    assert.equal(environment.PATH, "/a:/p");
  } finally {
    w.cleanup();
  }
});

test("an ended agent's leftover worktree and branch are released at the next start and its row is cleared", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    w.core.endAgent(ctx(w.core, w.owner), "developer-1");
    w.git.removed = [];
    w.git.deleted = [];
    await w.reopen().adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1", SHA]]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
    assert.deepEqual(w.launcher.status().cleanupFailed, []);
  } finally {
    w.cleanup();
  }
});

test("a worktree git refuses to remove keeps its row, is listed by status after a restart, and is retried until it goes", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    w.git.removeOk = false;
    await assert.rejects(w.launcher.spawn("developer"));
    const fresh = w.reopen();
    assert.deepEqual(fresh.status().cleanupFailed, [
      {
        agentId: "developer-1",
        reason: "the worktree could not be removed without force",
        worktreePath: "/tmp/work/developer-1",
      },
    ]);
    await fresh.adoptAll();
    assert.equal(
      fresh.status().cleanupFailed.length,
      1,
      "still refused, still listed",
    );
    w.git.removeOk = true;
    await fresh.adoptAll();
    assert.deepEqual(fresh.status().cleanupFailed, []);
    assert.deepEqual(w.git.deleted.at(-1), ["capstan/developer-1", SHA]);
  } finally {
    w.cleanup();
  }
});

test("a restart closes the recorded old pane even when adoption did not register it, and lists it when it cannot be closed", async () => {
  const w = await world();
  try {
    await launched(w);
    const oldPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const fresh = new StubAdapter();
    const launcher = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    fresh.adoptErrors.set(oldPane, new HerdrError("timeout", "slow"));
    const result = await launcher.restartPm();
    assert.equal(result.state, "started");
    assert.ok(
      fresh.calls.includes(`close:${oldPane}`),
      "the recorded pane was closed",
    );

    const again = new StubAdapter();
    const second = new Launcher({
      core: w.core,
      adapter: again,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const current = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    again.adoptErrors.set(current, new HerdrError("timeout", "slow"));
    again.closeError = new HerdrError("pane_close_failed", "busy");
    const last = await second.restartPm();
    assert.equal(last.state, "started");
    assert.deepEqual(second.status().orphanPanes, [
      { agentId: "pm-1", paneId: current },
    ]);
    assert.ok(
      again.calls.includes(`forget:${current}`),
      "the old pane no longer blocks the agent name",
    );
  } finally {
    w.cleanup();
  }
});

test("a hub that cannot be re-adopted for a transient reason is not replaced by a second hub; a vanished hub is", async () => {
  const w = await world();
  try {
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    const fresh = new StubAdapter();
    const make = (adapter: StubAdapter) =>
      new Launcher({
        core: w.core,
        adapter,
        config: config(),
        projectRoot: w.root,
        cliPath: "/c.js",
        socketPath: "/s",
        credential: w.owner,
        git: w.git,
        baseEnvironment: { PATH: "/usr/bin" },
      });
    fresh.adoptErrors.set(hub.paneId, new HerdrError("timeout", "slow"));
    const launcher = make(fresh);
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    fresh.entries.set(pmPane, { agent: "pm-1" });
    fresh.agentPanes.set("pm-1", pmPane);
    await assert.rejects(
      launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "hub_unavailable",
    );
    assert.deepEqual(
      w.core.fallbackPane(w.owner),
      hub,
      "the recorded hub is untouched",
    );
    assert.ok(
      !fresh.calls.some((c) => c.startsWith("workspace:capstan-watch")),
      "no second hub",
    );

    fresh.adoptErrors.set(hub.paneId, new PaneGone("gone"));
    const result = await launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.notDeepEqual(w.core.fallbackPane(w.owner), hub);
  } finally {
    w.cleanup();
  }
});

test("status lists an unfinished cleanup once, forgets it when the agent has ended, and an orphan pane is retried and dropped once it closes", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    (w.core as unknown as { endAgent: () => never }).endAgent = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(w.launcher.spawn("developer"));
    await assert.rejects(w.launcher.spawn("developer"));
    assert.equal(
      w.launcher.status().cleanupFailed.length,
      1,
      "one entry per agent, not one per attempt",
    );

    w.adapter.startError = undefined;
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const r1 = await w.launcher.restartPm();
    const r2 = await w.launcher.restartPm();
    assert.equal(
      w.launcher.status().orphanPanes.length,
      2,
      JSON.stringify([
        r1,
        r2,
        w.launcher.status().orphanPanes,
        w.adapter.calls,
      ]),
    );
    w.adapter.closeError = undefined;
    await w.launcher.adoptAll();
    assert.deepEqual(
      w.launcher.status().orphanPanes,
      [],
      "closed orphans are dropped",
    );
  } finally {
    w.cleanup();
  }
});

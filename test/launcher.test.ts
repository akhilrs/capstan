import assert from "node:assert/strict";
import { createHash } from "node:crypto";
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
import { Launcher, LauncherError, defaultGit } from "../src/launcher.js";
import { CSTAN_ALLOW_RULE } from "../src/prompts.js";
import {
  AgentPaneMismatch,
  PaneGone,
  PromptUnrecognized,
  buildAgentEnvironment,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import { PaneLost } from "../src/herdr/adapter.js";
import { ctx, projectInfo } from "./harness.js";
import { SHA, StubAdapter, StubGit } from "./launcher-stubs.js";

const hashOf = (name: string): string =>
  createHash("sha256").update(name).digest("hex");

function config(
  fallback = true,
  maxWorkers = 3,
  layout: Partial<CapstanConfig["layout"]> = {},
  pass: readonly string[] = [],
): CapstanConfig {
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
    configHash: hashOf(name),
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
      findingCheckSeconds: 1800,
    },
    limits: { maxWorkers },
    layout: {
      spawn: "tab",
      split: "auto",
      minPaneColumns: 60,
      minPaneRows: 12,
      ...layout,
    },
    env: { pass },
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
  reopen(syncRoles?: () => void): Launcher;
  cleanup(): void;
}

async function world(
  fallback = true,
  synced = true,
  maxWorkers = 3,
  layout: Partial<CapstanConfig["layout"]> = {},
  environment: {
    readonly pass?: readonly string[];
    readonly base?: Readonly<Record<string, string>>;
  } = {},
): Promise<World> {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-launcher-"));
  const stateDirectory = path.join(root, ".capstan", "state");
  const info = projectInfo();
  const core = await ControllerCore.open({ stateDirectory, project: info });
  const owner = info.ownerCredential;
  if (synced)
    core.syncRoleDefinitions(
      ctx(core, owner),
      ["pm:PM", "pm2:PM", "developer:Developer", "developer2:Developer"].map(
        (entry) => {
          const [name, kind] = entry.split(":") as [string, "PM" | "Developer"];
          return {
            name,
            kind,
            host: "claude",
            configHash: hashOf(name),
          };
        },
      ),
    );
  const adapter = new StubAdapter();
  const git = new StubGit();
  const events: World["events"] = [];
  const make = (syncRoles?: () => void): Launcher =>
    new Launcher({
      core,
      adapter,
      config: config(fallback, maxWorkers, layout, environment.pass),
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
        ...environment.base,
      },
      git,
      log: (event, details) => events.push({ event, details }),
      ...(syncRoles === undefined ? {} : { syncRoles }),
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
    assert.equal(result.branch, "capstan/developer-1-g1");
    assert.ok(
      w.adapter.calls.includes(`worktree:capstan/developer-1-g1:${SHA}`),
    );
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
      ["/tmp/work/developer-1", "capstan/developer-1-g1", SHA, true],
    );
  } finally {
    w.cleanup();
  }
});

test("launch on a project whose roles were never synced says so and leaves no seat, actor or agent behind", async () => {
  const w = await world(true, false);
  try {
    const seats = w.core.statusSnapshot().roles.length;
    await assert.rejects(
      w.launcher.launchPm(),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "role_not_synced" &&
        e.message.includes("pm") &&
        e.message.includes("cstan start"),
    );
    assert.equal(w.core.listAgents().length, 0);
    assert.equal(w.core.statusSnapshot().roles.length, seats);
    assert.equal(w.adapter.calls.length, 0);
  } finally {
    w.cleanup();
  }
});

test("spawn of a role that is not synced says so before it touches Herdr", async () => {
  const w = await world();
  try {
    await launched(w);
    w.core.syncRoleDefinitions(
      ctx(w.core, w.owner),
      ["pm:PM", "pm2:PM", "developer2:Developer"].map((entry) => {
        const [name, kind] = entry.split(":") as [string, "PM" | "Developer"];
        return {
          name,
          kind,
          host: "claude",
          configHash: hashOf(name),
        };
      }),
    );
    const calls = w.adapter.calls.length;
    const agents = w.core.listAgents().length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "role_not_synced" &&
        e.message.includes("developer"),
    );
    assert.equal(w.adapter.calls.length, calls);
    assert.equal(w.core.listAgents().length, agents);
  } finally {
    w.cleanup();
  }
});

test("a role whose definition changed is synced again on demand, and a sync that fails is named in the refusal", async () => {
  const w = await world();
  try {
    const stale = (hash: string) =>
      w.core.syncRoleDefinitions(
        ctx(w.core, w.owner),
        ["pm", "pm2", "developer", "developer2"].map((name) => ({
          name,
          kind: name.startsWith("pm")
            ? ("PM" as const)
            : ("Developer" as const),
          host: "claude",
          configHash: name === "pm" ? hash : hashOf(name),
        })),
      );
    stale("f".repeat(64));
    await assert.rejects(
      w.launcher.launchPm(),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "role_not_synced",
    );
    assert.equal(w.core.listAgents().length, 0);
    const failing = w.reopen(() => {
      throw new Error(
        "the ledger refused\nthe \u001b[31mchange\u001b[0m ".concat(
          "x".repeat(400),
        ),
      );
    });
    await assert.rejects(
      failing.launchPm(),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "role_not_synced" &&
        e.message.includes("the ledger refused the") &&
        e.message.includes("role_sync_failed") &&
        !/[\p{Cc}]/u.test(e.message) &&
        e.message.length < 450,
    );
    const healing = w.reopen(() => stale(hashOf("pm")));
    assert.equal((await healing.launchPm()).state, "started");
  } finally {
    w.cleanup();
  }
});

test("spawn refuses an unknown role, a PM role and a missing PM, without side effects", async () => {
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

test("several workers of one role get their own seats, ids, branches and worktrees up to the limit, and the next spawn names who is active", async () => {
  const w = await world();
  try {
    await launched(w);
    const one = await w.launcher.spawn("developer");
    const two = await w.launcher.spawn("developer");
    const three = await w.launcher.spawn("developer2");
    assert.deepEqual(
      [one.agentId, two.agentId, three.agentId],
      ["developer-1", "developer-2", "developer2-1"],
    );
    assert.notEqual(one.branch, two.branch);
    assert.notEqual(one.worktreePath, two.worktreePath);
    const seats = w.core
      .listAgents()
      .filter((a) => a.roleName === "developer")
      .map((a) => a.seatId);
    assert.deepEqual(seats, ["developer-seat", "developer-seat-2"]);
    const calls = w.adapter.calls.length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worker_limit" &&
        e.message.includes("3 of 3") &&
        e.message.includes("developer-1") &&
        e.message.includes("developer2-1"),
    );
    assert.equal(w.adapter.calls.length, calls, "nothing was started");
  } finally {
    w.cleanup();
  }
});

test("release ends a worker, closes its pane, removes its worktree and an unchanged branch, and frees its slot without reusing the id", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const released = await w.launcher.release(first.agentId);
    assert.equal(released.state, "released");
    assert.equal(released.agentId, "developer-1");
    assert.equal(released.branch, first.branch);
    assert.deepEqual(
      [released.paneClosed, released.worktreeRemoved, released.branchKept],
      [true, true, false],
    );
    assert.equal(w.core.agentRecord("developer-1")!.state, "ended");
    assert.ok(w.adapter.calls.includes(`close:${first.paneId}`));
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    assert.equal(w.git.deleted[0]![0], first.branch);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
      "the pane row is gone",
    );
    const again = await w.launcher.spawn("developer");
    assert.equal(again.agentId, "developer-2", "an ended id is not reused");
  } finally {
    w.cleanup();
  }
});

test("release keeps a branch that holds commits and reports a worktree git refused to remove", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.git.deleteOk = false;
    const kept = await w.launcher.release(first.agentId);
    assert.deepEqual(
      [kept.worktreeRemoved, kept.branchKept],
      [true, true],
      "the user merges a branch with commits",
    );
    const second = await w.launcher.spawn("developer");
    w.git.removeOk = false;
    const dirty = await w.launcher.release(second.agentId);
    assert.deepEqual(
      [dirty.paneClosed, dirty.worktreeRemoved, dirty.branchKept],
      [true, false, true],
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === second.agentId),
      true,
      "the row stays so the next start retries the removal",
    );
  } finally {
    w.cleanup();
  }
});

test("release keeps the pane row, worktree and branch when the pane will not close, and the next operation finishes the job", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const stuck = await w.launcher.release(first.agentId);
    assert.deepEqual(
      [stuck.paneClosed, stuck.worktreeRemoved, stuck.branchKept],
      [false, false, true],
    );
    assert.deepEqual(
      w.git.removed,
      [],
      "nothing was removed under an open pane",
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === first.agentId),
      true,
    );
    w.adapter.closeError = undefined;
    await w.launcher.spawn("developer");
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === first.agentId),
      false,
      "the retry cleared the row",
    );
  } finally {
    w.cleanup();
  }
});

test("an extra seat gets a dotted display name, so a role literally named like it still gets its own seat", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer");
    assert.throws(
      () =>
        w.core.createSeat(ctx(w.core, w.owner), {
          seatId: "other-seat",
          name: "developer.2",
          role: "Developer",
        }),
      "the extra seat already holds the display name developer.2",
    );
    const roleSeat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-2-seat",
      name: "developer-2",
      role: "Developer",
    });
    assert.equal(roleSeat.seatId, "developer-2-seat");
  } finally {
    w.cleanup();
  }
});

const PANE = { spawn: "pane" as const };

test("pane mode splits the worker into the PM's tab, records the new pane id and workspace, and stacks the next worker", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!;
    const first = await w.launcher.spawn("developer");
    assert.equal(first.placement, "pane");
    assert.equal(first.placementNote, undefined);
    assert.ok(
      w.adapter.calls.some((c) => c.endsWith(`:${pmPane.paneId}:right`)),
      "a wide PM pane is split to the right",
    );
    assert.notEqual(first.paneId, "w3:p1");
    const row = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.equal(row.paneId, first.paneId, "the ledger holds the new pane id");
    assert.equal(row.workspaceId, w.adapter.pmWorkspace);
    assert.equal(w.adapter.starts.at(-1)!.paneId, first.paneId);
    const second = await w.launcher.spawn("developer");
    assert.equal(second.placement, "pane");
    assert.ok(
      w.adapter.calls.some((c) => c.endsWith(`:${pmPane.paneId}:down`)),
      "the PM pane is now too narrow for another right split, so the next worker stacks below it",
    );
  } finally {
    w.cleanup();
  }
});

test("a fixed split direction is used as configured", async () => {
  const w = await world(true, true, 3, { spawn: "pane", split: "down" });
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    assert.ok(
      w.adapter.calls.some(
        (c) => c.startsWith("place:") && c.endsWith(":down"),
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("a worker stays a tab, with the reason, when nothing fits, the layout fails, the tab is zoomed or the move fails", async () => {
  const w = await world(true, true, 3, {
    spawn: "pane",
    minPaneColumns: 150,
    minPaneRows: 30,
  });
  try {
    await launched(w);
    const tooSmall = await w.launcher.spawn("developer");
    assert.equal(tooSmall.placement, "tab");
    assert.match(
      tooSmall.placementNote!,
      /no pane has room.*150 columns by 30 rows/,
    );
    assert.ok(!w.adapter.calls.some((c) => c.startsWith("place:")));
    const row = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.equal(row.paneId, tooSmall.paneId);
  } finally {
    w.cleanup();
  }
  const x = await world(true, true, 3, PANE);
  try {
    await launched(x);
    x.adapter.layoutError = new Error(
      "layout\nfailed \u001b[31mhard\u001b[0m " + "x".repeat(400),
    );
    const a = await x.launcher.spawn("developer");
    assert.equal(a.placement, "tab");
    assert.ok(
      a.placementNote!.startsWith(
        "the pane could not be placed (layout failed",
      ),
    );
    assert.ok(!/[\p{Cc}]/u.test(a.placementNote!));
    assert.ok(a.placementNote!.length < 260);
    x.adapter.layoutError = undefined;
    x.adapter.zoomed = true;
    const b = await x.launcher.spawn("developer");
    assert.equal(b.placement, "tab");
    assert.match(b.placementNote!, /zoomed/);
    x.adapter.zoomed = false;
    x.adapter.placeError = new Error("move refused");
    const c = await x.launcher.spawn("developer");
    assert.equal(c.placement, "tab");
    assert.match(c.placementNote!, /move refused/);
    const rowC = x.core
      .agentPanes(x.owner)
      .find((r) => r.agentId === c.agentId)!;
    assert.equal(rowC.paneId, c.paneId);
    assert.deepEqual(
      [a, b, c].map((r) => x.core.agentRecord(r.agentId)!.state),
      ["active", "active", "active"],
      "layout never fails a spawn",
    );
  } finally {
    x.cleanup();
  }
});

test("tab mode never asks for a layout", async () => {
  const w = await world();
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.placement, "tab");
    assert.ok(
      !w.adapter.calls.some(
        (c) => c.startsWith("layout:") || c.startsWith("place:"),
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("release closes a placed worker's new pane and removes its worktree, and a failed start after the move closes the new pane", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const released = await w.launcher.release(first.agentId);
    assert.equal(released.paneClosed, true);
    assert.ok(w.adapter.calls.includes(`close:${first.paneId}`));
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    w.adapter.startError = new Error("start failed");
    await assert.rejects(w.launcher.spawn("developer"));
    const placed = w.adapter.calls
      .filter((c) => c.startsWith("place:"))
      .at(-1)!;
    assert.ok(placed.startsWith("place:"));
    assert.ok(
      w.adapter.calls.some((c) => /^close:w\d+:p1\d$/.test(c)),
      "cleanup closed the pane at its new id",
    );
  } finally {
    w.cleanup();
  }
});

test("a pane lost in the move fails the spawn, and cleanup closes the one unregistered pane at the worktree path", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    w.adapter.placeError = new PaneLost("gone");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set("/tmp/work/developer-1", [
      { paneId: "w9:p42", workspaceId: w.adapter.pmWorkspace! },
    ]);
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) => e instanceof PaneLost,
    );
    assert.ok(
      w.adapter.calls.includes("close:w9:p42"),
      "the moved pane was found by its path and closed",
    );
    assert.equal(w.core.agentRecord("developer-1")!.state, "ended");
  } finally {
    w.cleanup();
  }
});

test("cleanup leaves panes at the worktree path alone when more than one matches or the recorded pane still exists", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    w.adapter.placeError = new PaneLost("gone");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set("/tmp/work/developer-1", [
      { paneId: "w9:p42", workspaceId: w.adapter.pmWorkspace! },
      { paneId: "w9:p43", workspaceId: w.adapter.pmWorkspace! },
    ]);
    await assert.rejects(w.launcher.spawn("developer"));
    assert.ok(!w.adapter.calls.includes("close:w9:p42"));
    assert.ok(!w.adapter.calls.includes("close:w9:p43"));
  } finally {
    w.cleanup();
  }
});

test("after a crash between the move and the ledger write, adoption closes the moved pane found at the worktree path in pane mode and leaves panes alone in tab mode", async () => {
  for (const [layout, closed] of [
    [{ spawn: "pane" as const }, true],
    [{ spawn: "tab" as const }, false],
  ] as const) {
    const w = await world(true, true, 3, layout);
    try {
      await launched(w);
      const first = await w.launcher.spawn("developer");
      w.core.recordAgentPane(ctx(w.core, w.owner), {
        agentId: first.agentId,
        workspaceId: "w7",
        paneId: "w7:p1",
        worktreePath: first.worktreePath,
        branch: first.branch,
        baseSha: SHA,
      });
      w.adapter.adoptErrors.set("w7:p1", new PaneGone("gone"));
      w.adapter.closeMissingThrows = true;
      w.adapter.strays.set(first.worktreePath, [
        { paneId: "w1:p99", workspaceId: w.adapter.pmWorkspace! },
      ]);
      await w.launcher.adoptAll();
      assert.equal(
        w.adapter.calls.includes("close:w1:p99"),
        closed,
        JSON.stringify(layout),
      );
      assert.equal(w.core.agentRecord(first.agentId)!.state, "ended");
    } finally {
      w.cleanup();
    }
  }
});

test("a pane at the worktree path in another workspace is the operator's and is never closed", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    w.adapter.placeError = new PaneLost("gone");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set("/tmp/work/developer-1", [
      { paneId: "w8:p1", workspaceId: "w8" },
    ]);
    await assert.rejects(w.launcher.spawn("developer"));
    assert.ok(!w.adapter.calls.includes("close:w8:p1"));
  } finally {
    w.cleanup();
  }
});

test("a placement note keeps no escape sequence of any kind and stays short even for combining marks", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const hostile = [
      "osc \u001b]0;evil title\u0007 end",
      "dcs \u001bPpayload\u001b\\ end",
      "c1 \u009b31mred\u009b0m end",
      "surrogate \ud800 end",
      "marks e" + "\u0301".repeat(5000),
      "unterminated \u001b]0;evil title",
      "8bit \u009d0;evil osc\u009c end",
      "8bit dcs \u0090payload\u009c end",
      "colon \u001b[38:2:255:0:0m red",
      "two byte \u001bc reset",
      "e".repeat(10) + "\u0301".repeat(500),
    ];
    for (const message of hostile) {
      w.adapter.layoutError = new Error(message);
      const result = await w.launcher.spawn("developer");
      assert.equal(result.placement, "tab");
      const note = result.placementNote!;
      assert.ok(
        !/evil title|payload|31m|0m|\u001b|\u009b|\ud800/.test(note),
        note,
      );
      assert.ok(note.length < 300, `${note.length}`);
      await w.launcher.release(result.agentId);
    }
  } finally {
    w.cleanup();
  }
});

test("a gone pane is not searched for when no move was interrupted: a pane recorded in the PM's workspace, or a normal release", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set(first.worktreePath, [
      { paneId: "w1:p77", workspaceId: w.adapter.pmWorkspace! },
    ]);
    // The worker's pane died after it was recorded in the PM's workspace; a release must not look for strays.
    w.adapter.tabPanes = w.adapter.tabPanes.filter(
      (p) => p.paneId !== first.paneId,
    );
    w.adapter.entries.delete(first.paneId);
    await w.launcher.release(first.agentId);
    assert.ok(!w.adapter.calls.some((c) => c.startsWith("panes-at:")));
    assert.ok(!w.adapter.calls.includes("close:w1:p77"));
  } finally {
    w.cleanup();
  }
  const x = await world(true, true, 3, PANE);
  try {
    await launched(x);
    const second = await x.launcher.spawn("developer");
    x.adapter.adoptErrors.set(second.paneId, new PaneGone("gone"));
    x.adapter.closeMissingThrows = true;
    x.adapter.strays.set(second.worktreePath, [
      { paneId: "w1:p77", workspaceId: x.adapter.pmWorkspace! },
    ]);
    await x.launcher.adoptAll();
    assert.ok(
      !x.adapter.calls.some((c) => c.startsWith("panes-at:")),
      "the row names the PM's workspace, so the pane died; nothing was mid-move",
    );
  } finally {
    x.cleanup();
  }
});

test("the branch is named for the generation, git must accept the name before anything is created, and an older row keeps the name it holds", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    assert.equal(first.branch, "capstan/developer-1-g1");
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === "developer-1")!
        .branch,
      "capstan/developer-1-g1",
    );
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "developer-1",
      workspaceId: "w3",
      paneId: first.paneId,
      worktreePath: first.worktreePath,
      branch: "capstan/developer-1",
      baseSha: SHA,
    });
    await w.launcher.release("developer-1");
    assert.equal(
      w.git.deleted.at(-1)![0],
      "capstan/developer-1",
      "cleanup uses the recorded name, whichever it is",
    );
    w.git.branchNamesValid = false;
    const calls = w.adapter.calls.length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) => e instanceof LauncherError && e.code === "invalid_branch",
    );
    assert.ok(
      !w.adapter.calls.slice(calls).some((c) => c.startsWith("worktree:")),
      "no worktree was created for a name git refuses",
    );
    assert.equal(w.core.agentRecord("developer-2")!.state, "ended");
  } finally {
    w.cleanup();
  }
});

test("spawn can start a worker's worktree and branch at a given commit, and refuses a base that is not a full id", async () => {
  const w = await world();
  try {
    await launched(w);
    const at = "c".repeat(40);
    const first = await w.launcher.spawn("developer", { baseSha: at });
    assert.ok(w.adapter.calls.includes(`worktree:${first.branch}:${at}`));
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === first.agentId)!
        .baseSha,
      at,
    );
    for (const bad of ["abc", "C".repeat(40), `${at} `, ""])
      await assert.rejects(
        w.launcher.spawn("developer", { baseSha: bad }),
        (e: unknown) => e instanceof LauncherError && e.code === "invalid_base",
        JSON.stringify(bad),
      );
    await w.launcher.release(first.agentId);
    assert.deepEqual(
      w.git.deleted.at(-1),
      [first.branch, at],
      "the review branch is deleted while it still points at the reviewed commit",
    );
  } finally {
    w.cleanup();
  }
});

test("a freed seat is reused before a new one is created", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer");
    await w.launcher.release("developer-1");
    const third = await w.launcher.spawn("developer");
    assert.equal(third.agentId, "developer-3");
    assert.equal(w.core.agentRecord("developer-3")!.seatId, "developer-seat");
  } finally {
    w.cleanup();
  }
});

test("release refuses an unknown agent, the PM and an agent that was already released, and reports a cleanup that cannot end the agent", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const code = (error: unknown) =>
      error instanceof LauncherError ? error.code : String(error);
    await assert.rejects(
      w.launcher.release("nobody-9"),
      (e: unknown) => code(e) === "unknown_agent",
    );
    await assert.rejects(
      w.launcher.release("pm-1"),
      (e: unknown) => code(e) === "kind_not_releasable",
    );
    const endAgent = w.core.endAgent.bind(w.core);
    (w.core as unknown as { endAgent: () => never }).endAgent = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(
      w.launcher.release(first.agentId),
      (e: unknown) =>
        code(e) === "release_blocked" &&
        /still holds authority/.test((e as Error).message),
    );
    assert.equal(w.core.agentRecord(first.agentId)!.state, "active");
    (w.core as unknown as { endAgent: typeof endAgent }).endAgent = endAgent;
    await w.launcher.release(first.agentId);
    assert.deepEqual(
      w.launcher.status().cleanupFailed,
      [],
      "a later successful release clears the failure",
    );
    await assert.rejects(
      w.launcher.release(first.agentId),
      (e: unknown) => code(e) === "agent_not_active",
    );
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
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
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
    assert.equal(retry.branch, "capstan/developer-2-g1");
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

test("a cleanup that cannot end the agent leaves it active, touches nothing else, and worker_limit names the reason", async () => {
  const w = await world(true, true, 1);
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
        e.code === "worker_limit" &&
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

test("two concurrent spawns at a limit of one end with one agent and one refusal", async () => {
  const w = await world(true, true, 1);
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
    assert.equal((rejected.reason as LauncherError).code, "worker_limit");
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
      branch: "capstan/developer-1-g1",
      baseSha: SHA,
    });
    w.git.byBranch.set("capstan/developer-1-g1", "/tmp/found/by/branch");
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/found/by/branch"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
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
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
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
    assert.deepEqual(w.git.deleted.at(-1), ["capstan/developer-1-g1", SHA]);
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
  const w = await world(true, true, 1);
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

test("a leftover row without a worktree path is reported as a record waiting to be cleaned up", async () => {
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
      workspaceId: "w9",
      paneId: "w9:p1",
      worktreePath: null,
      branch: null,
      baseSha: null,
    });
    w.core.endAgent(ctx(w.core, w.owner), "developer-1");
    assert.deepEqual(w.launcher.status().cleanupFailed, [
      {
        agentId: "developer-1",
        reason: "a record of an ended agent is waiting to be cleaned up",
      },
    ]);
  } finally {
    w.cleanup();
  }
});

test("a pane Herdr no longer knows counts as closed, never as an orphan", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.closeError = new HerdrError("pane_not_found", "no such pane");
    const result = await w.launcher.restartPm();
    assert.equal(result.state, "started");
    assert.deepEqual(w.launcher.status().orphanPanes, []);
  } finally {
    w.cleanup();
  }
});

test("a project path with a colon is refused because cstan could not be found on PATH", async () => {
  const w = await world();
  try {
    const odd = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: config(),
      projectRoot: path.join(w.root, "a:b"),
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const result = await odd.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason!, /colon/);
    assert.equal(
      w.core.listAgents().filter((a) => a.state === "active").length,
      0,
    );
  } finally {
    w.cleanup();
  }
});

test("a SHA-256 repository is named as unsupported instead of reported as having no commit", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-sha256-"));
  try {
    const init = spawnSync(
      "git",
      ["init", "-q", "--object-format=sha256", root],
      { encoding: "utf8" },
    );
    if (init.status !== 0) return; // this git cannot make one; nothing to check
    spawnSync(
      "git",
      [
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "x",
      ],
      { cwd: root },
    );
    assert.throws(
      () => defaultGit(root).headSha(),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "git_error" &&
        /SHA-256/.test(e.message),
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("an orphan pane is kept in the ledger, so a new launcher after a daemon restart still lists and retries it", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    await w.launcher.restartPm();
    const pane = w.launcher.status().orphanPanes[0]!.paneId;
    const afterRestart = w.reopen();
    assert.deepEqual(afterRestart.status().orphanPanes, [
      { agentId: "pm-1", paneId: pane },
    ]);
    w.adapter.closeError = undefined;
    await afterRestart.adoptAll();
    assert.deepEqual(afterRestart.status().orphanPanes, []);
    assert.ok(w.adapter.calls.includes(`close:${pane}`));
  } finally {
    w.cleanup();
  }
});

test("when git cannot say whether a worktree exists the branch and the row are kept, never deleted", async () => {
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
      branch: "capstan/developer-1-g1",
      baseSha: SHA,
    });
    w.git.byBranchError = new LauncherError(
      "git_error",
      "git could not list the worktrees",
    );
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.deleted, []);
    assert.deepEqual(w.git.removed, []);
    assert.ok(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      "the row stays",
    );
    assert.ok(eventNames(w).includes("worktree_unknown"));
    w.git.byBranchError = undefined;
    w.git.byBranch.set("capstan/developer-1-g1", "/tmp/found");
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/found"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("variables listed in env.pass reach the PM, every worker and every reviewer, and nothing else does", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      pass: ["NEXORA_API_KEY", "EXTRA_SETTING"],
      base: {
        NEXORA_API_KEY: "key-value-1",
        EXTRA_SETTING: "two words",
        UNLISTED: "no",
      },
    },
  );
  try {
    await w.launcher.launchPm();
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer2", { baseSha: "a".repeat(40) });
    assert.equal(w.adapter.starts.length, 3);
    for (const start of w.adapter.starts) {
      assert.equal(start.environment!.NEXORA_API_KEY, "key-value-1");
      assert.equal(start.environment!.EXTRA_SETTING, "two words");
      assert.equal(start.environment!.UNLISTED, undefined);
      assert.equal(start.environment!.SECRET, undefined);
    }
    assert.equal(
      w.adapter.lastShellEnvironment?.NEXORA_API_KEY,
      undefined,
      "the watch pane does not get passed variables",
    );
    const logged = JSON.stringify(w.events);
    assert.ok(
      !logged.includes("key-value-1"),
      "a passed value is never logged",
    );
  } finally {
    w.cleanup();
  }
});

test("a listed variable that is not set is named in the answer and the log, and the others still pass", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      pass: ["NEXORA_API_KEY", "MISSING_ONE", "EMPTY_ONE"],
      base: { NEXORA_API_KEY: "key-value-2", EMPTY_ONE: "" },
    },
  );
  try {
    const launched = await w.launcher.launchPm();
    assert.deepEqual(launched.missingEnv, ["MISSING_ONE", "EMPTY_ONE"]);
    assert.match(
      launched.warning ?? "",
      /^MISSING_ONE, EMPTY_ONE are listed in \[env\] pass but not set where the daemon was started/,
    );
    const spawned = await w.launcher.spawn("developer");
    assert.deepEqual(spawned.missingEnv, ["MISSING_ONE", "EMPTY_ONE"]);
    assert.equal(w.adapter.starts[1]!.environment!.EMPTY_ONE, undefined);
    const again = await w.launcher.launchPm();
    assert.equal(again.state, "running");
    assert.equal(again.missingEnv, undefined, "nothing was started");
    assert.equal(again.warning, undefined);
    const restarted = await w.launcher.restartPm();
    assert.deepEqual(restarted.missingEnv, ["MISSING_ONE", "EMPTY_ONE"]);
    assert.equal(w.adapter.starts[1]!.environment!.MISSING_ONE, undefined);
    assert.equal(
      w.adapter.starts[1]!.environment!.NEXORA_API_KEY,
      "key-value-2",
    );
    assert.ok(
      w.events.some(
        (e) =>
          e.event === "env_pass_missing" &&
          JSON.stringify(e.details) === '{"names":["MISSING_ONE","EMPTY_ONE"]}',
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("with nothing listed the answers carry no missingEnv and an unacceptable passed value fails naming only the variable", async () => {
  const plain = await world();
  try {
    assert.equal((await plain.launcher.launchPm()).missingEnv, undefined);
  } finally {
    plain.cleanup();
  }
  const bad = await world(
    true,
    true,
    3,
    {},
    {
      pass: ["BAD_VALUE", "ALSO_MISSING"],
      base: { BAD_VALUE: "secret\u0007text" },
    },
  );
  try {
    const result = await bad.launcher.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason ?? "", /BAD_VALUE/);
    assert.equal(
      result.missingEnv,
      undefined,
      "a failed launch started nothing",
    );
    assert.ok(!JSON.stringify(result).includes("secret"));
    assert.ok(!JSON.stringify(bad.events).includes("secret"));
  } finally {
    bad.cleanup();
  }
});

test("overlapping operations are told apart: only the one that started an agent reports the missing variable", async () => {
  const w = await world(true, true, 3, {}, { pass: ["MISSING_ONE"], base: {} });
  try {
    const [first, second] = await Promise.all([
      w.launcher.launchPm(),
      w.launcher.launchPm(),
    ]);
    assert.equal(first.state, "started");
    assert.deepEqual(first.missingEnv, ["MISSING_ONE"]);
    assert.equal(second.state, "running");
    assert.equal(second.missingEnv, undefined);
    assert.equal(second.warning, undefined);
    assert.equal(
      w.events.filter((e) => e.event === "env_pass_missing").length,
      1,
    );
  } finally {
    w.cleanup();
  }
});

test("observe reads the recorded pane of an active agent, sanitizes the text and reports Herdr's state; an ended or paneless agent is refused", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const spawned = await w.launcher.spawn("developer");
    const paneId = w.core
      .agentPanes(w.owner)
      .find((row) => row.agentId === spawned.agentId)!.paneId!;
    w.adapter.screens.set(
      paneId,
      "\u001b[31mnpm test\u001b[0m\r\nFAIL expected 3 got 4\u0007",
    );
    w.adapter.observation = "working";
    const seen = await w.launcher.observe(spawned.agentId, 25);
    assert.equal(seen.text, "npm test\nFAIL expected 3 got 4");
    assert.equal(seen.agentStatus, "working");
    assert.equal(seen.kind, "Developer");
    assert.equal(seen.roleName, "developer");
    assert.deepEqual(w.adapter.screenReads.at(-1), { paneId, lines: 25 });
    w.adapter.unreadablePanes.add(paneId);
    await assert.rejects(
      w.launcher.observe(spawned.agentId, 25),
      (error: Error) =>
        error instanceof LauncherError && error.code === "pane_unreadable",
    );
    w.adapter.unreadablePanes.delete(paneId);
    await assert.rejects(
      w.launcher.observe("nobody", 40),
      (error: Error) =>
        error instanceof LauncherError && error.code === "agent_not_active",
    );
    await w.launcher.release(spawned.agentId);
    await assert.rejects(
      w.launcher.observe(spawned.agentId, 40),
      (error: Error) =>
        error instanceof LauncherError && error.code === "agent_not_active",
    );
  } finally {
    w.cleanup();
  }
});

/** An accepted report by the worker, using the token it was started with. */
function reportAs(
  w: World,
  agentId: string,
  startIndex: number,
  commit: string,
  summary: string,
): void {
  const token = w.adapter.starts[startIndex]!.environment!.CAPSTAN_TOKEN!;
  const row = w.core.agentPanes(w.owner).find((r) => r.agentId === agentId)!;
  const { record } = w.core.recordAgentReport(ctx(w.core, token), {
    commitSha: commit,
    summary,
    evidence: {
      generation: 1,
      branch: row.branch!,
      baseSha: row.baseSha!,
      commitExists: true,
      branchTip: commit,
      isAncestorOfTip: true,
      isAncestorOfBase: false,
      checkedAt: "2026-10-01T00:00:00.000Z",
    },
  });
  assert.equal(record.state, "accepted");
}

function promptOf(w: World, startIndex: number): string {
  const args = w.adapter.starts[startIndex]!.args;
  return readFileSync(
    args[args.indexOf("--append-system-prompt-file") + 1]!,
    "utf8",
  );
}

test("replace releases a running worker and starts a new agent of the same role from its last accepted report, seeded from the ledger, and sends nothing again", async () => {
  const w = await world();
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    const sent = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: old.agentId,
      body: "write the parser",
    }).messageId;
    const commit = "c".repeat(40);
    w.git.reachable.add(commit);
    w.git.tips.set(old.branch, "d".repeat(40));
    reportAs(w, old.agentId, 1, commit, "parser written");
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(result.predecessor, old.agentId);
    assert.equal(result.baseSha, commit);
    assert.equal(result.baseSource, "predecessor");
    assert.equal(result.replacementRecorded, true);
    assert.deepEqual(result.cancelledMessageIds, [sent]);
    assert.notEqual(result.agentId, old.agentId, "a new id");
    assert.equal(w.core.agentRecord(old.agentId)!.state, "ended");
    assert.equal(w.core.agentRecord(result.agentId)!.state, "active");
    assert.equal(w.core.agentRecord(result.agentId)!.roleName, "developer");
    assert.equal(w.core.isAgentReplaced(old.agentId), true);
    assert.deepEqual(
      w.core.messagesFor(result.agentId),
      [],
      "nothing is sent to the replacement",
    );
    assert.equal(w.core.message(sent)!.state, "cancelled");
    const prompt = promptOf(w, 2);
    assert.match(
      prompt,
      /===== replacement seed, generated from the ledger =====/,
    );
    assert.ok(
      prompt.includes(
        `You replace agent ${old.agentId} (role developer), which has ended`,
      ),
    );
    assert.ok(
      prompt.includes(
        `Your branch starts at ${commit}, the predecessor's last accepted report`,
      ),
    );
    assert.ok(prompt.includes(`(tip ${"d".repeat(40)})`), prompt);
    assert.ok(prompt.includes(JSON.stringify("write the parser")));
    assert.ok(
      prompt.includes(
        `commit ${commit} on ${old.branch}: ${JSON.stringify("parser written")}`,
      ),
    );
    assert.ok(prompt.includes(`${sent} from operator [queued]`));
    await assert.rejects(
      w.launcher.replace(old.agentId),
      (error: Error) =>
        error instanceof LauncherError && error.code === "already_replaced",
    );
  } finally {
    w.cleanup();
  }
});

test("replace falls back to the project's HEAD when the predecessor has no accepted report, its commit is not reachable or its branch is gone", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const a = await w.launcher.replace(first.agentId);
    assert.equal(a.state, "started");
    if (a.state !== "started") return;
    assert.equal(a.baseSource, "head");
    assert.equal(a.baseSha, w.git.head);
    assert.match(
      promptOf(w, 2),
      /the project's HEAD \(the predecessor had no accepted report that could be used\)/,
    );
    const commit = "e".repeat(40);
    reportAs(w, a.agentId, 2, commit, "work");
    w.git.reachable.clear();
    const b = await w.launcher.replace(a.agentId);
    assert.equal(b.state, "started");
    if (b.state !== "started") return;
    assert.equal(b.baseSource, "head", "an unreachable commit is not used");
    w.git.reachable.add(commit);
    const c = await w.launcher.replace(b.agentId);
    assert.equal(
      c.state === "started" && c.baseSource,
      "head",
      "no report by that agent",
    );
  } finally {
    w.cleanup();
  }
});

test("replace works on an agent that already ended, releases nothing and answers with no predecessor worktree", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    const removedBefore = [...w.git.removed];
    const result = await w.launcher.replace(first.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(result.predecessorWorktreeRemoved, null);
    assert.deepEqual(result.cancelledMessageIds, []);
    assert.deepEqual(w.git.removed, removedBefore);
    assert.ok(
      promptOf(w, 2).includes(
        `You replace agent ${first.agentId} (role developer), which has ended`,
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("replace stops when the old pane will not close, so two agents never work on one task, and a pane that is already gone is fine", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const blocked = await w.launcher.replace(first.agentId);
    assert.equal(blocked.state, "blocked");
    assert.ok("reason" in blocked);
    if ("reason" in blocked)
      assert.match(
        blocked.reason,
        new RegExp(
          `${first.agentId} was released but its pane is still open: close it in Herdr, then run cstan replace ${first.agentId} again`,
        ),
      );
    assert.equal(w.adapter.starts.length, 2, "no replacement was started");
    assert.equal(w.core.isAgentReplaced(first.agentId), false);
    w.adapter.closeError = undefined;
    const second = await w.launcher.spawn("developer");
    w.adapter.closeMissingThrows = true;
    w.adapter.entries.delete(second.paneId);
    const result = await w.launcher.replace(second.agentId);
    assert.equal(
      result.state,
      "started",
      "a pane that is already gone is not an error",
    );
  } finally {
    w.cleanup();
  }
});

test("replace refuses the PM, an unknown agent, a running integration, a second replacement and a concurrent one, before anything is released", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await assert.rejects(
      w.launcher.replace("pm-1"),
      (e: Error) =>
        e instanceof LauncherError && e.code === "kind_not_replaceable",
    );
    await assert.rejects(
      w.launcher.replace("nobody"),
      (e: Error) => e instanceof LauncherError && e.code === "unknown_agent",
    );
    assert.ok(first);
  } finally {
    w.cleanup();
  }
  const v = await world();
  try {
    await launched(v);
    const target = await v.launcher.spawn("developer");
    const original = v.core.runningIntegrations.bind(v.core);
    v.core.runningIntegrations = () => [{ integrationId: "x" } as never];
    await assert.rejects(
      v.launcher.replace(target.agentId),
      (e: Error) =>
        e instanceof LauncherError && e.code === "integration_running",
    );
    assert.equal(
      v.core.agentRecord(target.agentId)!.state,
      "active",
      "nothing was released",
    );
    v.core.runningIntegrations = original;
    const gate = v.launcher.replace(target.agentId);
    await assert.rejects(
      v.launcher.replace(target.agentId),
      (e: Error) => e instanceof LauncherError && e.code === "replace_running",
    );
    assert.equal((await gate).state, "started");
  } finally {
    v.cleanup();
  }
});

test("replace names the released agent when the replacement cannot start, and a rerun finds it ended and cancels nothing twice", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const queued = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: first.agentId,
      body: "x",
    }).messageId;
    w.git.branchNamesValid = false;
    await assert.rejects(
      w.launcher.replace(first.agentId),
      (e: Error) =>
        e instanceof LauncherError &&
        e.code === "replacement_not_started" &&
        e.message.includes(
          `${first.agentId} was released but the replacement could not start`,
        ) &&
        e.message.includes(`run cstan replace ${first.agentId} again`),
    );
    assert.equal(w.core.agentRecord(first.agentId)!.state, "ended");
    assert.equal(w.core.message(queued)!.state, "cancelled");
    assert.equal(w.core.isAgentReplaced(first.agentId), false);
    w.git.branchNamesValid = true;
    const rerun = await w.launcher.replace(first.agentId);
    assert.equal(rerun.state, "started");
    assert.equal(w.core.isAgentReplaced(first.agentId), true);
  } finally {
    w.cleanup();
  }
});

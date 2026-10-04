import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  AgentPaneMismatch,
  PaneGone,
  PromptUnrecognized,
  buildAgentEnvironment,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import { Launcher, LauncherError } from "../src/launcher.js";
import { CSTAN_ALLOW_RULE } from "../src/prompts.js";
import { ctx } from "./harness.js";
import { SHA, StubAdapter } from "./launcher-stubs.js";
import {
  config,
  eventNames,
  hashOf,
  launched,
  world,
} from "./launcher-harness.js";

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

test("adoption sweeps stale rows and crashed spawns with and without a row, and keeps the row of a vanished watch pane", async () => {
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
    assert.deepEqual(
      w.core.fallbackPane(w.owner),
      fallback,
      "a gone watch pane keeps its row so the project workspace can get a new watch tab",
    );
    assert.ok(eventNames(w).includes("fallback_pane_gone"));
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

test("a project workspace that fails to open fails the PM start and leaves no agent, row or pane, and spawn needs a launched PM", async () => {
  const w = await world();
  try {
    w.adapter.runError = new Error("cannot run");
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason!, /project workspace could not be opened/);
    assert.equal(w.core.fallbackPane(w.owner), undefined);
    assert.equal(
      w.core.listAgents().filter((a) => a.state === "active").length,
      0,
    );
    assert.ok(
      w.adapter.calls.some((c) => c.startsWith("close:")),
      "the workspace made for the PM is closed again",
    );
    assert.ok(
      w.adapter.calls.some((c) => /^close:w1:p10\d$/.test(c)),
      "the watch tab's pane is closed again, so no tab is left unrecorded",
    );
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "pm_not_launched",
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

test("a hub that cannot be re-adopted for a transient reason is not replaced by a second hub; a vanished watch pane is made again in the same workspace", async () => {
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
      !fresh.calls.some((c) => c.startsWith("workspace:")),
      "no second hub",
    );

    fresh.adoptErrors.set(hub.paneId, new PaneGone("gone"));
    const result = await launcher.spawn("developer");
    assert.equal(result.state, "started");
    const remade = w.core.fallbackPane(w.owner)!;
    assert.equal(
      remade.workspaceId,
      hub.workspaceId,
      "a vanished watch pane is made again in the same project workspace",
    );
    assert.ok(
      fresh.calls.some((c) => c === `tab:${hub.workspaceId}:watch:worker`),
    );
    assert.ok(!fresh.calls.some((c) => c.startsWith("workspace:")));
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

test("capturePrompt and answerPrompt resolve the active agent's pane, pass the hash and answer through, log the keys, and refuse an agent that is not active", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const spawned = await w.launcher.spawn("developer");
    const paneId = w.core
      .agentPanes(w.owner)
      .find((row) => row.agentId === spawned.agentId)!.paneId!;
    assert.deepEqual(await w.launcher.capturePrompt(spawned.agentId), {
      captured: false,
      reason: "prompt_unrecognized",
    });
    assert.ok(w.adapter.calls.includes(`capture:${paneId}`));
    let before = 0;
    const outcome = await w.launcher.answerPrompt(spawned.agentId, {
      promptSha: "a".repeat(64),
      answer: { kind: "option", number: 1 },
      beforeType: () => {
        before += 1;
      },
    });
    assert.deepEqual(outcome, { typed: true, keys: ["enter"] });
    assert.equal(before, 1);
    assert.deepEqual(w.adapter.answered, [
      {
        paneId,
        promptSha: "a".repeat(64),
        answer: { kind: "option", number: 1 },
      },
    ]);
    for (const call of [
      () => w.launcher.capturePrompt("nobody"),
      () =>
        w.launcher.answerPrompt("nobody", {
          promptSha: "a".repeat(64),
          answer: { kind: "esc" },
          beforeType: () => undefined,
        }),
    ])
      await assert.rejects(
        call(),
        (error: Error) =>
          error instanceof LauncherError && error.code === "agent_not_active",
      );
    assert.equal(w.adapter.answered.length, 1);
  } finally {
    w.cleanup();
  }
});

test("workspaces are labelled with the project and every started pane reports project, role and agent", async () => {
  const w = await world();
  try {
    const project = path.basename(w.root);
    await launched(w);
    const spawned = await w.launcher.spawn("developer");
    assert.equal(spawned.state, "started");
    assert.deepEqual(
      w.adapter.calls.filter((c) => c.startsWith("workspace:")),
      [`workspace:${project}:PM`],
      "one workspace per project, named after it, whose root pane is the PM's",
    );
    const hub = w.core.fallbackPane(w.owner)!;
    assert.ok(
      w.adapter.calls.includes(`tab:${hub.workspaceId}:watch:worker`),
      "the watch shell is a tab of that workspace",
    );
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === "pm-1")!.workspaceId,
      hub.workspaceId,
      "the PM lives in the project workspace",
    );
    assert.ok(w.adapter.created.includes(`${project} · developer-1`));
    assert.ok(w.adapter.labels.includes(`${hub.workspaceId}:${project}`));
    assert.ok(
      w.adapter.labels.includes(`${hub.workspaceId}:t1:pm`),
      "the PM's tab is named pm",
    );
    const pm = w.adapter.metadata.find((m) => m.tokens.agent === "pm-1")!;
    assert.deepEqual(pm.tokens, {
      project,
      role: "pm",
      agent: "pm-1",
    });
    const dev = w.adapter.metadata.find(
      (m) => m.tokens.agent === "developer-1",
    )!;
    assert.deepEqual(dev.tokens, {
      project,
      role: "developer",
      agent: "developer-1",
    });
    assert.equal(
      w.adapter.metadata.filter((m) => m.tokens.agent === undefined).length,
      2,
      "the PM's and the watch workspace carry the project token; a worker adds none",
    );
  } finally {
    w.cleanup();
  }
});

test("a failure to report metadata is logged and never fails a start", async () => {
  const w = await world();
  try {
    w.adapter.metadataError = new Error("herdr is busy");
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "started");
    assert.ok(w.events.some((e) => e.event === "describe_failed"));
  } finally {
    w.cleanup();
  }
});

test("an adopted PM keeps the project workspace name, an old PM workspace gets its project label, and metadata is reported again", async () => {
  const w = await world();
  try {
    const project = path.basename(w.root);
    await launched(w);
    const spawned = await w.launcher.spawn("developer");
    assert.equal(spawned.state, "started");
    w.adapter.labels.length = 0;
    w.adapter.metadata.length = 0;
    w.adapter.entries.clear();
    w.adapter.agentPanes.clear();
    await w.reopen().adoptAll();
    const worker = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.ok(
      !w.adapter.labels.some((l) => l.endsWith("developer-1")),
      "a worker's workspace may be the PM's, so it is never relabelled",
    );
    assert.ok(worker.workspaceId !== null);
    const pm = w.core.agentPanes(w.owner).find((r) => r.agentId === "pm-1")!;
    const hub = w.core.fallbackPane(w.owner)!;
    assert.equal(pm.workspaceId, hub.workspaceId);
    assert.ok(
      !w.adapter.labels.includes(`${pm.workspaceId}:${project} · pm`),
      "a PM in the project workspace never renames it to a pm label",
    );
    assert.ok(w.adapter.metadata.some((m) => m.tokens.agent === "developer-1"));

    // A PM started before the project workspace existed still has a workspace of its own.
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "pm-1",
      workspaceId: "w77",
      paneId: pm.paneId,
      worktreePath: null,
      branch: null,
      baseSha: null,
    });
    w.adapter.entries.clear();
    w.adapter.agentPanes.clear();
    w.adapter.labels.length = 0;
    await w.reopen().adoptAll();
    assert.ok(w.adapter.labels.includes(`w77:${project} · pm`));
  } finally {
    w.cleanup();
  }
});

test("adoption renames a watch workspace of an earlier layout to the project name", async () => {
  const w = await world();
  try {
    const project = path.basename(w.root);
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    w.adapter.entries.clear();
    w.adapter.agentPanes.clear();
    w.adapter.labels.length = 0;
    await w.reopen().adoptAll();
    assert.ok(w.adapter.labels.includes(`${hub.workspaceId}:${project}`));
  } finally {
    w.cleanup();
  }
});

test("a hub made again while a PM is already running closes its empty root pane, and keeps the watch tab", async () => {
  const w = await world();
  try {
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const fresh = new StubAdapter();
    fresh.adoptErrors.set(hub.paneId, new PaneGone("gone"));
    fresh.tabErrors.add(hub.workspaceId!);
    await fresh.createWorkspace({ cwd: "/x", label: "other", role: "worker" });
    fresh.calls.length = 0;
    fresh.entries.set(pmPane, { agent: "pm-1" });
    fresh.agentPanes.set("pm-1", pmPane);
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
    const result = await launcher.launchPm();
    assert.equal(result.state, "running");
    assert.equal(result.hub, "opened");
    const made = w.core.fallbackPane(w.owner)!;
    assert.notEqual(made.workspaceId, hub.workspaceId);
    const root = fresh.calls.find((c) => c.startsWith("workspace:"))!;
    assert.ok(root.endsWith(":PM"));
    assert.ok(
      fresh.calls.some((c) => c === `close:${made.workspaceId}:p1`),
      "the empty root pane is closed",
    );
    assert.ok(
      fresh.calls.some((c) => c === `tab:${made.workspaceId}:watch:worker`),
    );
  } finally {
    w.cleanup();
  }
});

test("a watch tab that cannot be made again for a transient reason keeps the row and makes no second hub", async () => {
  const w = await world();
  try {
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const fresh = new StubAdapter();
    fresh.adoptErrors.set(hub.paneId, new PaneGone("gone"));
    fresh.tabFailure = new HerdrError("timeout", "slow");
    fresh.entries.set(pmPane, { agent: "pm-1" });
    fresh.agentPanes.set("pm-1", pmPane);
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
    const result = await launcher.launchPm();
    assert.equal(result.hub, "failed");
    assert.deepEqual(w.core.fallbackPane(w.owner), hub, "the row is kept");
    assert.ok(!fresh.calls.some((c) => c.startsWith("workspace:")));
  } finally {
    w.cleanup();
  }
});

test("a PM start that fails before it takes the new workspace's root pane closes that pane", async () => {
  const w = await world();
  try {
    w.adapter.promptError = new Error("disk full");
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason!, /disk full/);
    assert.equal(
      w.adapter.calls.filter((c) => c === "close:w1:p1").length,
      1,
      "the empty root pane is closed once",
    );
    assert.ok(w.core.fallbackPane(w.owner), "the watch tab stays recorded");
  } finally {
    w.cleanup();
  }
});

test("without a worktree configuration spawn never runs setup", async () => {
  const calls: string[] = [];
  const w = await world(
    true,
    true,
    3,
    {},
    {
      runSetup: async (command) => {
        calls.push(command);
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    assert.deepEqual(calls, []);
  } finally {
    w.cleanup();
  }
});

test("operatorEnvironment is the filtered setup environment with no CAPSTAN_ variable and no cstan wrapper directory", async () => {
  const w = await world();
  try {
    const wrapperDirectory = path.join(w.root, ".capstan", "bin");
    const launcher = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: config(true, 3, {}, ["KEPT_NAME"]),
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, "control.sock"),
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: {
        PATH: `${wrapperDirectory}:/usr/bin:/bin`,
        HOME: "/home/x",
        CAPSTAN_TOKEN: "agent-token-value",
        CAPSTAN_SOCKET: "/tmp/socket",
        KEPT_NAME: "kept",
        SECRET: "no",
      },
      git: w.git,
    });
    const environment = launcher.operatorEnvironment();
    assert.deepEqual(
      Object.keys(environment).filter((name) => name.startsWith("CAPSTAN_")),
      [],
    );
    assert.equal(environment.PATH, "/usr/bin:/bin");
    assert.equal(environment.KEPT_NAME, "kept");
    assert.equal(environment.HOME, "/home/x");
    assert.equal(environment.SECRET, undefined);
    assert.ok(!JSON.stringify(environment).includes("agent-token-value"));
    // Building it does not create the wrapper, which only agent environments need.
    assert.equal(existsSync(wrapperDirectory), false);
    assert.equal(launcher.inFlightOperations(), 0);
  } finally {
    w.cleanup();
  }
});

test("operatorEnvironment drops PATH when only the wrapper directory was on it", async () => {
  const w = await world();
  try {
    const launcher = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: config(),
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, "control.sock"),
      credential: w.owner,
      baseEnvironment: { PATH: path.join(w.root, ".capstan", "bin") },
      git: w.git,
    });
    assert.equal(launcher.operatorEnvironment().PATH, undefined);
  } finally {
    w.cleanup();
  }
});

test("interrupt sends exactly one Esc to a working agent, nothing to an idle one, and is refused without prompt_relay", async () => {
  const off = await world();
  try {
    await off.launcher.launchPm();
    const spawned = await off.launcher.spawn("developer");
    await assert.rejects(
      off.launcher.interrupt(spawned.agentId),
      (error: Error) =>
        error instanceof LauncherError && error.code === "not_configured",
    );
    assert.deepEqual(off.adapter.interrupts, []);
  } finally {
    off.cleanup();
  }
  const w = await world(true, true, 3, {}, { promptRelay: true });
  try {
    await w.launcher.launchPm();
    const spawned = await w.launcher.spawn("developer");
    const paneId = w.core
      .agentPanes(w.owner)
      .find((row) => row.agentId === spawned.agentId)!.paneId!;
    assert.equal(await w.launcher.interrupt(spawned.agentId), false);
    assert.deepEqual(w.adapter.interrupts, [], "an idle agent gets no key");
    w.adapter.workingPanes.add(paneId);
    assert.equal(await w.launcher.interrupt(spawned.agentId), true);
    assert.deepEqual(w.adapter.interrupts, ["esc"]);
    assert.ok(
      w.events.some(
        (e) =>
          e.event === "prompt_relay_key" &&
          e.details.key === "esc" &&
          e.details.agentId === spawned.agentId,
      ),
      "the key is logged",
    );
    await assert.rejects(
      w.launcher.interrupt("nobody"),
      (error: Error) =>
        error instanceof LauncherError && error.code === "agent_not_active",
    );
    assert.deepEqual(w.adapter.interrupts, ["esc"]);
  } finally {
    w.cleanup();
  }
});

test("a spawn that fails while choosing the branch name cleans up as if no branch was recorded", async () => {
  const w = await world();
  try {
    await launched(w);
    await assert.rejects(
      w.launcher.spawn("developer", { task: "plan-1/nope" }),
      (e: unknown) => e instanceof LauncherError && e.code === "unknown_task",
    );
    w.git.tips.set("taken", SHA);
    await assert.rejects(
      w.launcher.spawn("developer", { branch: "taken" }),
      (e: unknown) => e instanceof LauncherError && e.code === "branch_in_use",
    );
    assert.ok(!w.git.byBranchQueries.includes(""));
    assert.deepEqual(w.git.deleted, []);
    assert.ok(!eventNames(w).includes("branch_kept"));
  } finally {
    w.cleanup();
  }
});

import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  CONFIG_FILE_NAME,
  loadCapstanConfig,
  type CapstanConfig,
  type ResolvedResearcher,
  type ResolvedRole,
} from "../src/config/capstan-config.js";
import {
  PaneGone,
  PromptUnrecognized,
  ShellNotReady,
  PaneLost,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import { Launcher, LauncherError, runSetupCommand } from "../src/launcher.js";
import { CSTAN_ALLOW_RULE } from "../src/prompts.js";
import {
  RESEARCHER_REQUIRED_DENY,
  researcherRuleProblems,
} from "../src/researcher-policy.js";
import { ctx } from "./harness.js";
import { SHA } from "./launcher-stubs.js";
import {
  PANE,
  SETUP,
  config,
  hashOf,
  launched,
  promptOf,
  reportAs,
  type World,
  world,
} from "./launcher-harness.js";

test("spawn starts one worker in its own worktree with its token, prompt, worker profile and a base sha, and records the row", async () => {
  const w = await world();
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.equal(result.agentId, "developer-1");
    assert.equal(result.branch, "chore/developer-1-developer");
    assert.ok(
      w.adapter.calls.includes(`worktree:chore/developer-1-developer:${SHA}`),
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
      ["/tmp/work/developer-1", "chore/developer-1-developer", SHA, true],
    );
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
    const second = await w.launcher.spawn("developer2");
    assert.equal(second.placement, "pane");
    assert.ok(
      w.adapter.calls.some((c) => c.endsWith(`:${first.paneId}:down`)),
      "the next worker stacks below the first worker, in the column on the PM's right",
    );
    assert.ok(
      !w.adapter.calls.some((c) => c.endsWith(`:${pmPane.paneId}:down`)),
      "the PM pane is never split down",
    );
    const layout = w.adapter.tabPanes.find((p) => p.paneId === pmPane.paneId)!;
    assert.equal(layout.height, 50, "the PM pane keeps the full height");
    assert.equal(layout.width, 120, "the PM pane keeps 60% of the width");
  } finally {
    w.cleanup();
  }
});

test("the PM keeps pm_width_percent of its tab when the first worker is placed", async () => {
  const w = await world(true, true, 3, { spawn: "pane", pmWidthPercent: 70 });
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    assert.deepEqual(w.adapter.keeps, [0.7]);
    await w.launcher.spawn("developer2");
    assert.deepEqual(w.adapter.keeps, [0.7, 0.5]);
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
    assert.equal(first.branch, "chore/developer-1-developer");
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === "developer-1")!
        .branch,
      "chore/developer-1-developer",
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
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "agent_start_failed",
    );
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer-1")!.state,
      "ended",
    );
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["chore/developer-1-developer", SHA]]);
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
    assert.equal(retry.branch, "chore/developer-2-developer");
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

test("a worker on a Codex host starts with full access, its worktree trusted and the prompt as instructions", async () => {
  const w = await world(true, true, 3, {}, { hostOf: { developer: "codex" } });
  mkdirSync("/tmp/work/developer-1", { recursive: true });
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    const start = w.adapter.starts.find((s) => s.name === "developer-1")!;
    assert.equal(start.kind, "codex");
    assert.deepEqual(start.args.slice(0, 4), [
      "--sandbox",
      "danger-full-access",
      "--ask-for-approval",
      "never",
    ]);
    assert.ok(!start.args.includes(CSTAN_ALLOW_RULE));
    assert.ok(!start.args.includes("--append-system-prompt-file"));
    const real = realpathSync("/tmp/work/developer-1");
    assert.ok(start.args.includes(`projects."${real}".trust_level="trusted"`));
    const instructions = start.args.find((a) =>
      a.startsWith("developer_instructions="),
    )!;
    assert.match(instructions, /cstan ack <message-id>/);
    assert.ok(!instructions.includes("\n"));
    assert.equal(w.adapter.starts[0]!.kind, "claude", "the PM stays on Claude");
  } finally {
    w.cleanup();
  }
});

test("a worker on an OMP host starts with every tool approved and the prompt file appended", async () => {
  const w = await world(true, true, 3, {}, { hostOf: { developer: "omp" } });
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    const start = w.adapter.starts.find((s) => s.name === "developer-1")!;
    assert.equal(start.kind, "omp");
    assert.deepEqual(start.args.slice(0, 2), ["--approval-mode", "yolo"]);
    const file = start.args[start.args.indexOf("--append-system-prompt") + 1]!;
    assert.match(readFileSync(file, "utf8"), /cstan ack <message-id>/);
    assert.ok(!start.args.includes(CSTAN_ALLOW_RULE));
  } finally {
    w.cleanup();
  }
});

test("the Supervisor does not take a worker's place: it starts when every worker place is taken, and is not counted against the limit", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    const dev = await w.launcher.spawn("developer");
    assert.equal(dev.state, "started");
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) => e instanceof LauncherError && e.code === "worker_limit",
    );
    const supervisor = await w.launcher.spawn("supervisor");
    assert.equal(supervisor.state, "started");
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worker_limit" &&
        /1 of 1 workers are active \(developer-1\)/.test(e.message),
      "the Supervisor is not listed or counted among the workers",
    );
  } finally {
    w.cleanup();
  }
});

test("the Architect takes no worker place unless count_toward_worker_limit is true, and its prompt differs from a developer's", async () => {
  const exempt = await world(
    true,
    true,
    1,
    {},
    { architect: { counts: false } },
  );
  try {
    await launched(exempt);
    const architect = await exempt.launcher.spawn("architect");
    assert.equal(architect.state, "started");
    const dev = await exempt.launcher.spawn("developer");
    assert.equal(dev.state, "started", "the developer still has its place");
    await assert.rejects(
      exempt.launcher.spawn("developer2"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worker_limit" &&
        /1 of 1 workers are active \(developer-1\)/.test(e.message),
    );
    const prompts = exempt.adapter.prompts;
    assert.ok(prompts.some((t) => t.includes("You are the architect")));
    assert.ok(
      prompts.some((t) => t.includes("the architect named in it can answer")),
    );
    assert.ok(
      prompts.some((t) => t.includes("Planned work")),
      "the PM prompt carries the plan section",
    );
  } finally {
    exempt.cleanup();
  }
  const counted = await world(
    true,
    true,
    1,
    {},
    { architect: { counts: true } },
  );
  try {
    await launched(counted);
    await counted.launcher.spawn("architect");
    await assert.rejects(
      counted.launcher.spawn("developer"),
      (e: unknown) => e instanceof LauncherError && e.code === "worker_limit",
    );
  } finally {
    counted.cleanup();
  }
});

test("without the Architect no prompt mentions plans", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    for (const text of w.adapter.prompts) {
      assert.ok(!/architect/i.test(text), "no architect text");
      assert.ok(!text.includes("cstan plan"), "no plan commands");
    }
  } finally {
    w.cleanup();
  }
});

test("an enabled Researcher gets the researcher prompt and the MCP arguments, and the PM prompt gains the research section", async () => {
  const w = await world(true, true, 3, {}, { researcher: { enabled: true } });
  try {
    await launched(w);
    const researcher = await w.launcher.spawn("researcher");
    assert.equal(researcher.state, "started");
    const start = w.adapter.starts.at(-1)!;
    assert.ok(start.args.includes("--mcp-config"));
    assert.ok(start.args.includes("--strict-mcp-config"));
    assert.ok(start.args.includes(CSTAN_ALLOW_RULE));
    const prompt = promptOf(w, w.adapter.starts.length - 1);
    assert.match(prompt, /You are the researcher of a Capstan delivery team/);
    assert.match(prompt, /docs\/research\//);
    assert.match(prompt, /capstan-researcher\/1\.0 \(test\)/);
    const pm = promptOf(w, 0);
    assert.match(pm, /Web research \(the Researcher is enabled/);
    await w.launcher.spawn("developer");
    const dev = promptOf(w, w.adapter.starts.length - 1);
    assert.ok(!dev.includes("You are the researcher"));
    assert.ok(!dev.includes("Web research"));
  } finally {
    w.cleanup();
  }
});

test("the cstan allow rule the launcher appends passes the researcher rule check", () => {
  const role = {
    name: "researcher",
    kind: "Developer",
    permissionMode: "default",
    allow: [CSTAN_ALLOW_RULE],
    deny: [...RESEARCHER_REQUIRED_DENY],
  } as unknown as ResolvedRole;
  assert.deepEqual(
    researcherRuleProblems(role, {
      enabled: true,
      role: "researcher",
      outputDir: "docs/research",
    } as unknown as ResolvedResearcher),
    [],
  );
});

test("with [researcher] disabled a role named researcher gets the plain developer prompt and the PM has no research section", async () => {
  const w = await world(true, true, 3, {}, { researcher: { enabled: false } });
  try {
    await launched(w);
    await w.launcher.spawn("researcher");
    const prompt = promptOf(w, w.adapter.starts.length - 1);
    assert.ok(!prompt.includes("You are the researcher"));
    assert.ok(!prompt.includes("Output contract"));
    assert.ok(!/Web research/.test(promptOf(w, 0)));
  } finally {
    w.cleanup();
  }
});

test("setup runs once in the new worktree after it is created and before the worker starts, for a worker and the architect, not the PM", async () => {
  const calls: Array<[string, string, number]> = [];
  const startsAtSetup: number[] = [];
  const holder: { w?: World } = {};
  const w = await world(
    true,
    true,
    3,
    {},
    {
      architect: { counts: false },
      worktree: SETUP,
      runSetup: async (command, cwd, timeoutMs) => {
        calls.push([command, cwd, timeoutMs]);
        startsAtSetup.push(holder.w!.adapter.starts.length);
        return { status: "ok" };
      },
    },
  );
  holder.w = w;
  try {
    await launched(w);
    assert.deepEqual(calls, []);
    const first = await w.launcher.spawn("developer");
    assert.deepEqual(calls, [["npm ci", first.worktreePath, 7000]]);
    assert.deepEqual(startsAtSetup, [1], "only the PM had started");
    assert.ok(
      w.adapter.calls.findIndex((c) => c.startsWith("worktree:")) <
        w.adapter.calls.findIndex((c) => c.startsWith("place:")) ||
        !w.adapter.calls.some((c) => c.startsWith("place:")),
    );
    await w.launcher.spawn("architect");
    assert.equal(calls.length, 2);
  } finally {
    w.cleanup();
  }
});

test("a failing setup rejects with worktree_setup_failed naming the exit code and output, and removes pane, worktree, branch and agent", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async () => ({
        status: "failed",
        exitCode: 3,
        output: "npm ERR! \u001b[31mboom\u001b[0m\nsecond line",
      }),
    },
  );
  try {
    await launched(w);
    const startsBefore = w.adapter.starts.length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worktree_setup_failed" &&
        /developer-1/.test(e.message) &&
        /npm ci/.test(e.message) &&
        /exit code 3/.test(e.message) &&
        /npm ERR! boom second line/.test(e.message),
    );
    assert.equal(w.adapter.starts.length, startsBefore);
    assert.ok(w.adapter.calls.some((c) => c.startsWith("close:")));
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["chore/developer-1-developer", SHA]]);
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

test("setup time is not charged to the step budget", async () => {
  let clock = 1_000_000;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async () => {
        clock += 5 * 60_000;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const slow = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: {
        ...config(),
        worktree: SETUP,
      } as CapstanConfig,
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, ".capstan", "state", "control.sock"),
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: { PATH: "/usr/bin:/bin" },
      git: w.git,
      now: () => clock,
      runSetup: async () => {
        clock += 5 * 60_000;
        return { status: "ok" };
      },
    });
    const result = await slow.spawn("developer");
    assert.equal(result.state, "started");
  } finally {
    w.cleanup();
  }
});

test("runSetupCommand leaves a succeeding command's effect in the directory", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "capstan-setup-"));
  try {
    const outcome = await runSetupCommand("touch made.txt", dir, 10_000, {
      PATH: "/usr/bin:/bin",
    });
    assert.deepEqual(outcome, { status: "ok" });
    assert.ok(existsSync(path.join(dir, "made.txt")));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("runSetupCommand reports exit code and a capped output tail on failure", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "capstan-setup-"));
  try {
    const outcome = await runSetupCommand(
      "echo oops >&2; exit 4",
      dir,
      10_000,
      { PATH: "/usr/bin:/bin" },
    );
    assert.equal(outcome.status, "failed");
    if (outcome.status === "failed") {
      assert.equal(outcome.exitCode, 4);
      assert.match(outcome.output, /oops/);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("runSetupCommand kills the whole process group on timeout", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "capstan-setup-"));
  try {
    const started = Date.now();
    const outcome = await runSetupCommand(
      "sleep 30 & echo $! > child.pid; wait",
      dir,
      1000,
      { PATH: "/usr/bin:/bin" },
    );
    assert.deepEqual(outcome, { status: "timeout" });
    assert.ok(Date.now() - started < 10_000);
    const pid = Number(readFileSync(path.join(dir, "child.pid"), "utf8"));
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.throws(() => process.kill(pid, 0), /ESRCH/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a worktree section loaded from capstan.toml reaches runSetup with its command, cwd and timeout", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-launcher-toml-"));
  const calls: Array<[string, string, number]> = [];
  let w: World | undefined;
  try {
    writeFileSync(
      path.join(directory, CONFIG_FILE_NAME),
      `schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[worktree]
setup = "make deps"
setup_timeout_seconds = 42
`,
      { mode: 0o600 },
    );
    const loaded = loadCapstanConfig(directory);
    assert.ok(loaded.worktree !== undefined);
    w = await world(
      true,
      true,
      3,
      {},
      {
        worktree: loaded.worktree,
        runSetup: async (command, cwd, timeoutMs) => {
          calls.push([command, cwd, timeoutMs]);
          return { status: "ok" };
        },
      },
    );
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.deepEqual(calls, [["make deps", result.worktreePath, 42_000]]);
  } finally {
    w?.cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("setup runs with the agents' filtered environment: no daemon-only variable, the [env] pass variables present, no agent token", async () => {
  const base = mkdtempSync(path.join(tmpdir(), "capstan-setup-env-"));
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: { setup: "env > setup-env.txt", setupTimeoutSeconds: 10 },
      pass: ["PASSED_VALUE"],
      base: { PASSED_VALUE: "yes", DAEMON_ONLY: "leak" },
    },
  );
  try {
    mkdirSync(path.join(base, "developer-1"));
    w.adapter.worktreeBase = base;
    await launched(w);
    const result = await w.launcher.spawn("developer");
    const text = readFileSync(
      path.join(result.worktreePath, "setup-env.txt"),
      "utf8",
    );
    assert.match(text, /^PASSED_VALUE=yes$/m);
    assert.match(text, /^HOME=\/home\/x$/m);
    assert.doesNotMatch(text, /DAEMON_ONLY|SECRET/);
    assert.doesNotMatch(text, /CAPSTAN_TOKEN/);
  } finally {
    w.cleanup();
    rmSync(base, { recursive: true, force: true });
  }
});

test("the Operator is refused while disabled, takes no worker place unless it counts, and has its own prompt", async () => {
  const off = await world(
    true,
    true,
    3,
    {},
    {
      operator: { counts: false, enabled: false },
    },
  );
  try {
    await launched(off);
    await assert.rejects(
      off.launcher.spawn("operator"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "operator_disabled",
    );
  } finally {
    off.cleanup();
  }
  const exempt = await world(
    true,
    true,
    1,
    {},
    {
      operator: { counts: false, enabled: true },
    },
  );
  try {
    await launched(exempt);
    const operator = await exempt.launcher.spawn("operator");
    assert.equal(operator.state, "started");
    const dev = await exempt.launcher.spawn("developer");
    assert.equal(dev.state, "started", "the developer still has its place");
    const prompts = exempt.adapter.prompts;
    assert.ok(prompts.some((t) => t.includes("You are the operator")));
    assert.ok(prompts.some((t) => t.includes("the Operator is enabled")));
    assert.ok(
      !prompts.some(
        (t) =>
          t.includes("You are developer (Developer)") &&
          t.includes("cstan op propose"),
      ),
      "an ordinary developer prompt does not mention the operator",
    );
  } finally {
    exempt.cleanup();
  }
  const counted = await world(
    true,
    true,
    1,
    {},
    {
      operator: { counts: true, enabled: true },
    },
  );
  try {
    await launched(counted);
    await counted.launcher.spawn("operator");
    await assert.rejects(
      counted.launcher.spawn("developer"),
      (e: unknown) => e instanceof LauncherError && e.code === "worker_limit",
    );
  } finally {
    counted.cleanup();
  }
});

test("a plain Developer role named operator with no [operator] table spawns with the ordinary developer prompt", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    { operator: { counts: false, enabled: false, table: false } },
  );
  try {
    await launched(w);
    const spawned = await w.launcher.spawn("operator");
    assert.equal(spawned.state, "started");
    const prompt = w.adapter.prompts.at(-1)!;
    assert.ok(prompt.includes("(Developer) on a Capstan delivery team"));
    assert.ok(!prompt.includes("You are the operator"));
    assert.ok(!prompt.includes("cstan op "));
  } finally {
    w.cleanup();
  }
});

test("a failed spawn names the failing step and the real error, whatever the adapter threw", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new PromptUnrecognized("no bare prompt");
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "spawn_failed" &&
        e.message === "step start: no bare prompt",
    );
    const failed = w.events.find((e) => e.event === "spawn_failed")!;
    assert.deepEqual(failed.details, {
      agentId: "developer-1",
      step: "start",
      error: "step start: no bare prompt",
    });
    w.adapter.startError = new HerdrError("agent_start_failed", "no binary");
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "agent_start_failed" &&
        e.message === "step start: no binary",
    );
    w.adapter.startError = undefined;
    w.adapter.worktreeError = new Error("herdr is gone");
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.message === "step worktree: herdr is gone",
    );
  } finally {
    w.cleanup();
  }
});

test("a worker start is tried again only for an unready prompt or shell, at most three times, and is never retried for another error", async () => {
  const w = await world();
  try {
    await launched(w);
    const base = w.adapter.startAttempts;

    w.adapter.startErrors = [new PromptUnrecognized("first")];
    const second = await w.launcher.spawn("developer");
    assert.equal(second.state, "started");
    assert.equal(w.adapter.startAttempts - base, 2, "succeeds on the 2nd");
    assert.deepEqual(w.sleeps, [1500]);
    assert.equal(w.events.filter((e) => e.event === "start_retry").length, 1);

    const before = w.adapter.startAttempts;
    w.adapter.startErrors = [
      new ShellNotReady("a"),
      new PromptUnrecognized("b"),
      new ShellNotReady("c"),
    ];
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.message === "step start: c",
    );
    assert.equal(w.adapter.startAttempts - before, 3, "fails after 3");
    assert.deepEqual(w.sleeps, [1500, 1500, 1500]);

    const other = w.adapter.startAttempts;
    w.adapter.startErrors = [new Error("boom")];
    await assert.rejects(w.launcher.spawn("developer"), /step start: boom/);
    assert.equal(
      w.adapter.startAttempts - other,
      1,
      "another error: 1 attempt",
    );
  } finally {
    w.cleanup();
  }
});

test("a retry never runs after the agent started", async () => {
  const w = await world();
  try {
    await launched(w);
    const original = w.adapter.startAgent.bind(w.adapter);
    w.adapter.startAgent = async (input) => {
      await original(input);
      throw new PromptUnrecognized("after the start");
    };
    const before = w.adapter.starts.length;
    await assert.rejects(w.launcher.spawn("developer"), /after the start/);
    assert.equal(w.adapter.starts.length - before, 1);
  } finally {
    w.cleanup();
  }
});

async function approvedPackagePlan(
  w: World,
  architectStart: number,
  packages: Array<Record<string, unknown>>,
): Promise<string> {
  const planId = w.core.openPlan(ctx(w.core, w.owner), {
    tier: "normal",
    title: "names",
  }).planId;
  w.core.submitPlan(
    ctx(w.core, w.adapter.starts[architectStart]!.environment!.CAPSTAN_TOKEN!),
    {
      planId,
      bodyJson: JSON.stringify({ summary: "s", packages }),
      baseSha: "a".repeat(40),
      review: false,
    },
  );
  return planId;
}

test("spawn --task names the branch after the package: its type, the plan and package ids (or the linked Nexora id) and its title", async () => {
  const w = await world(true, true, 5);
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    const planId = await approvedPackagePlan(w, 1, [
      { id: "conventions", title: "Conventions module", type: "fix" },
      { id: "plain", title: "Plain one" },
    ]);
    const typed = await w.launcher.spawn("developer", {
      task: `${planId}/conventions`,
    });
    assert.equal(typed.branch, `fix/${planId}-conventions-conventions-module`);
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === typed.agentId)!
        .branch,
      typed.branch,
    );
    assert.ok(w.adapter.calls.includes(`worktree:${typed.branch}:${SHA}`));
    w.core.linkExternal(ctx(w.core, w.owner), {
      refKind: "package",
      refId: `${planId}/plain`,
      externalId: "NX-12",
    });
    const linked = await w.launcher.spawn("developer", {
      task: `${planId}/plain`,
      title: "Own words",
    });
    assert.equal(linked.branch, "feat/NX-12-own-words");
    await assert.rejects(
      w.launcher.spawn("developer", { task: `${planId}/nope` }),
      (e: unknown) => e instanceof LauncherError && e.code === "unknown_task",
    );
  } finally {
    w.cleanup();
  }
});

test("a requirement task names the branch after its ref id; --type wins over the default", async () => {
  const w = await world();
  try {
    await launched(w);
    const one = await w.launcher.spawn("developer", {
      task: "req-7",
      type: "refactor",
    });
    assert.equal(one.branch, "refactor/req-7-req-7");
  } finally {
    w.cleanup();
  }
});

test("renameBranchForTask renames an ad-hoc branch with no commit and no report; the worktree row, the ledger and a later report follow", async () => {
  const w = await world();
  try {
    await launched(w);
    const spawned = await w.launcher.spawn("developer");
    w.git.tips.set(spawned.branch, SHA);
    const result = await w.launcher.renameBranchForTask(
      spawned.agentId,
      "req-9",
    );
    assert.deepEqual(result, { branch: "feat/req-9-req-9", renamed: true });
    assert.deepEqual(w.git.renames, [[spawned.branch, "feat/req-9-req-9"]]);
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === spawned.agentId)!
        .branch,
      "feat/req-9-req-9",
    );
    const commit = "c".repeat(40);
    w.git.reachable.add(commit);
    w.git.tips.set("feat/req-9-req-9", commit);
    reportAs(w, spawned.agentId, 1, commit, "done");
    const later = await w.launcher.renameBranchForTask(
      spawned.agentId,
      "req-10",
    );
    assert.equal(later.renamed, false);
    assert.match(later.note!, /^branch kept: feat\/req-9-req-9 /);
  } finally {
    w.cleanup();
  }
});

test("renameBranchForTask keeps the task-based name a spawn with --task/--type/--title gave", async () => {
  const w = await world();
  try {
    await launched(w);
    const spawned = await w.launcher.spawn("developer", {
      task: "req-supervision-banner",
      type: "fix",
      title: "show real supervision state",
    });
    assert.equal(
      spawned.branch,
      "fix/req-supervision-banner-show-real-supervision-state",
    );
    w.git.tips.set(spawned.branch, SHA);
    const result = await w.launcher.renameBranchForTask(
      spawned.agentId,
      "req-supervision-banner",
    );
    assert.equal(result.renamed, false);
    assert.equal(result.branch, spawned.branch);
    assert.match(result.note!, /already has a task-based name/);
    assert.deepEqual(w.git.renames, []);
  } finally {
    w.cleanup();
  }
});

test("renameBranchForTask after an ad-hoc spawn with a title keeps the title as the slug, not the ref id", async () => {
  const w = await world();
  try {
    await launched(w);
    const spawned = await w.launcher.spawn("developer", {
      title: "clean up temp dirs",
    });
    w.git.tips.set(spawned.branch, SHA);
    const result = await w.launcher.renameBranchForTask(
      spawned.agentId,
      "req-tmp-leak",
    );
    assert.deepEqual(result, {
      branch: "feat/req-tmp-leak-clean-up-temp-dirs",
      renamed: true,
    });
  } finally {
    w.cleanup();
  }
});

test("renameBranchForTask keeps a branch that has a commit and gives a taken name a -2", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.git.tips.set(first.branch, "d".repeat(40));
    const kept = await w.launcher.renameBranchForTask(first.agentId, "req-9");
    assert.equal(kept.renamed, false);
    assert.equal(kept.branch, first.branch);
    assert.match(kept.note!, /^branch kept: chore\/developer-1-developer /);
    assert.deepEqual(w.git.renames, []);
    const second = await w.launcher.spawn("developer");
    w.git.tips.set(second.branch, SHA);
    w.git.tips.set("feat/req-9-req-9", "e".repeat(40));
    const named = await w.launcher.renameBranchForTask(second.agentId, "req-9");
    assert.deepEqual(named, { branch: "feat/req-9-req-9-2", renamed: true });
  } finally {
    w.cleanup();
  }
});

test("a spawn stores its task and folded title on every pane row; a spawn without them stores null; a hub re-record keeps them", async () => {
  const w = await world();
  try {
    await launched(w);
    const tasked = await w.launcher.spawn("developer", {
      task: "req-1",
      title: "  Fix   it ",
    });
    const plain = await w.launcher.spawn("developer");
    const rows = (): Map<
      string,
      { taskRef: string | null; taskTitle: string | null }
    > => new Map(w.core.agentPanes(w.owner).map((r) => [r.agentId, r]));
    assert.equal(rows().get(tasked.agentId)!.taskRef, "req-1");
    assert.equal(rows().get(tasked.agentId)!.taskTitle, "Fix it");
    assert.equal(rows().get(plain.agentId)!.taskRef, null);
    assert.equal(rows().get(plain.agentId)!.taskTitle, null);
    const row = rows().get(tasked.agentId)!;
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: tasked.agentId,
      workspaceId: "w9",
      paneId: "w9:p1",
      worktreePath: "/tmp/work/other",
      branch: tasked.branch,
      baseSha: SHA,
    });
    assert.equal(rows().get(tasked.agentId)!.taskRef, row.taskRef);
    assert.equal(rows().get(tasked.agentId)!.taskTitle, "Fix it");

    const long = await w.launcher.spawn("developer", {
      task: "req-2",
      title: "x".repeat(250),
    });
    assert.equal(rows().get(long.agentId)!.taskTitle, "x".repeat(200));
  } finally {
    w.cleanup();
  }
});

test("a title with a control character is refused before any agent exists", async () => {
  const w = await world();
  try {
    await launched(w);
    const before = w.core.listAgents().length;
    await assert.rejects(
      w.launcher.spawn("developer", { task: "req-1", title: "bad\u0007" }),
      (e: unknown) => e instanceof LauncherError && e.code === "invalid_task",
    );
    assert.equal(w.core.listAgents().length, before);
  } finally {
    w.cleanup();
  }
});

test("a spawn with a task that fails at worktree creation leaves no pane row and no active agent", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.worktreeError = new Error("no worktree");
    await assert.rejects(
      w.launcher.spawn("developer", { task: "req-1", title: "Fix it" }),
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer-1")!.state,
      "ended",
    );
  } finally {
    w.cleanup();
  }
});

test("replace gives the successor's own row the predecessor's task and keeps the predecessor's branch", async () => {
  const w = await world();
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer", {
      task: "req-1",
      title: "Fix it",
    });
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    const row = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === result.agentId)!;
    assert.equal(row.taskRef, "req-1");
    assert.equal(row.taskTitle, "Fix it");
    assert.equal(row.branch, old.branch);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === old.agentId),
      false,
    );
  } finally {
    w.cleanup();
  }
});

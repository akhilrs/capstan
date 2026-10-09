import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import {
  type CapstanConfig,
  type ResolvedWorktree,
} from "../src/config/capstan-config.js";
import { PaneLost } from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import {
  Launcher,
  LauncherError,
  type TeardownRunner,
} from "../src/launcher.js";
import { ctx } from "./harness.js";
import { SHA } from "./launcher-stubs.js";
import {
  PANE,
  SETUP,
  config,
  eventNames,
  launched,
  promptOf,
  reportAs,
  type World,
  world,
} from "./launcher-harness.js";

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
      (e: unknown) =>
        e instanceof LauncherError && e.message === "step place: gone",
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
        reason: "git could not remove the worktree",
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
    assert.deepEqual(w.git.deleted, [["chore/developer-1-developer", SHA]]);
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
        reason: "git could not remove the worktree",
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
    assert.deepEqual(w.git.deleted.at(-1), [
      "chore/developer-1-developer",
      SHA,
    ]);
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

test("replace moves the work packages of the predecessor to the replacement and the seed names them", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    const old = await w.launcher.spawn("developer");
    const other = await w.launcher.spawn("developer");
    const planId = w.core.openPlan(ctx(w.core, w.owner), {
      tier: "normal",
      title: "split",
    }).planId;
    w.core.submitPlan(
      ctx(w.core, w.adapter.starts[1]!.environment!.CAPSTAN_TOKEN!),
      {
        planId,
        bodyJson: JSON.stringify({
          summary: "s",
          packages: [
            {
              id: "wp1",
              title: "parser",
              owns: ["src/parser.ts"],
              acceptance: ["parses empty input"],
            },
            { id: "wp2", title: "other" },
          ],
        }),
        baseSha: "a".repeat(40),
        review: false,
      },
    );
    for (const packageId of ["wp1", "wp2"])
      w.core.assignPackage(ctx(w.core, w.owner), {
        planId,
        packageId,
        agentId: packageId === "wp1" ? old.agentId : other.agentId,
      });
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    const packages = w.core.planRecord(w.owner, planId)!.packages;
    assert.equal(
      packages.find((p) => p.packageId === "wp1")!.assigneeAgentId,
      result.agentId,
    );
    assert.equal(
      packages.find((p) => p.packageId === "wp2")!.assigneeAgentId,
      other.agentId,
      "a package of another agent is not touched",
    );
    const prompt = promptOf(w, 4);
    assert.ok(
      prompt.includes(
        `${planId}/wp1: "parser"; owns "src/parser.ts"; acceptance "parses empty input"`,
      ),
      prompt,
    );
    assert.ok(!prompt.includes(`${planId}/wp2`));
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

test("replace seeds from the head with the predecessor tip when the last accepted commit was amended away", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const tip = "d".repeat(40);
    w.git.tips.set(first.branch, tip);
    reportAs(w, first.agentId, 1, "e".repeat(40), "work");
    w.git.reachable.clear();
    const result = await w.launcher.replace(first.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(result.baseSource, "head");
    assert.equal(result.baseSha, w.git.head);
    assert.ok(promptOf(w, 2).includes(`(tip ${tip})`));
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

test("replace and PM restart rebuild the researcher prompt and the research section", async () => {
  const w = await world(true, true, 3, {}, { researcher: { enabled: true } });
  try {
    await launched(w);
    const old = await w.launcher.spawn("researcher");
    const first = promptOf(w, w.adapter.starts.length - 1);
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    const start = w.adapter.starts.at(-1)!;
    assert.ok(start.args.includes("--mcp-config"));
    const replaced = promptOf(w, w.adapter.starts.length - 1);
    assert.match(replaced, /You are the researcher of a Capstan delivery team/);
    assert.match(replaced, /docs\/research\//);
    assert.ok(first.includes("Output contract"));
    assert.ok(replaced.includes("Output contract"));
    await w.launcher.restartPm();
    assert.match(
      promptOf(w, w.adapter.starts.length - 1),
      /Web research \(the Researcher is enabled/,
    );
  } finally {
    w.cleanup();
  }
});

test("a timed-out setup rejects with worktree_setup_failed and the same cleanup", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async () => ({ status: "timeout" }),
    },
  );
  try {
    await launched(w);
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worktree_setup_failed" &&
        /timed out after 7s/.test(e.message),
    );
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["chore/developer-1-developer", SHA]]);
  } finally {
    w.cleanup();
  }
});

test("replace runs setup in the new worktree and reports a setup failure", async () => {
  let fail = false;
  const cwds: string[] = [];
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async (_command, cwd) => {
        cwds.push(cwd);
        return fail
          ? { status: "failed", exitCode: 1, output: "nope" }
          : { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    const replaced = await w.launcher.replace(old.agentId);
    assert.equal(replaced.state, "started");
    assert.equal(cwds.length, 2);
    if (replaced.state !== "started") return;
    fail = true;
    await assert.rejects(
      w.launcher.replace(replaced.agentId),
      (e: unknown) =>
        e instanceof LauncherError &&
        /worktree_setup_failed|setup/.test(String((e as Error).message)),
    );
  } finally {
    w.cleanup();
  }
});

const TEARDOWN: ResolvedWorktree = {
  setupTimeoutSeconds: 600,
  teardown: "rm -rf cache",
  teardownTimeoutSeconds: 9,
};

test("teardown runs once per cleanup, in the project root, before the worktree is removed", async () => {
  const calls: Array<[string, string, number]> = [];
  const holder: { w?: World } = {};
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async (command, cwd, timeoutMs) => {
        calls.push([command, cwd, timeoutMs]);
        holder.w!.git.order.push("teardown");
        return { status: "ok" };
      },
    },
  );
  holder.w = w;
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    assert.deepEqual(calls, [], "no teardown while the worker lives");
    await w.launcher.release(first.agentId);
    assert.deepEqual(calls, [["rm -rf cache", w.root, 9000]]);
    assert.deepEqual(w.git.order, ["teardown", "remove"]);
    assert.deepEqual(
      eventNames(w).filter((name) => name.startsWith("teardown")),
      ["teardown_started"],
    );
  } finally {
    w.cleanup();
  }
});

test("teardown runs with the agents' filtered environment plus the worktree path and agent id, and with no agent token or socket", async () => {
  const base = mkdtempSync(path.join(tmpdir(), "capstan-teardown-env-"));
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: {
        setupTimeoutSeconds: 600,
        teardown: "env > teardown-env.txt; pwd > teardown-pwd.txt",
        teardownTimeoutSeconds: 10,
      },
      pass: ["PASSED_VALUE"],
      base: { PASSED_VALUE: "yes", DAEMON_ONLY: "leak" },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    const text = readFileSync(path.join(w.root, "teardown-env.txt"), "utf8");
    assert.match(text, /^PASSED_VALUE=yes$/m);
    assert.match(
      text,
      new RegExp(`^CAPSTAN_WORKTREE_PATH=${first.worktreePath}$`, "m"),
    );
    assert.match(text, /^CAPSTAN_AGENT_ID=developer-1$/m);
    assert.doesNotMatch(text, /DAEMON_ONLY|SECRET/);
    assert.doesNotMatch(text, /CAPSTAN_TOKEN|CAPSTAN_SOCKET/);
    const names = text
      .split("\n")
      .map((line) => line.split("=")[0]!)
      .filter((name) => name.startsWith("CAPSTAN_"));
    assert.deepEqual(names.sort(), [
      "CAPSTAN_AGENT_ID",
      "CAPSTAN_WORKTREE_PATH",
    ]);
    assert.equal(
      readFileSync(path.join(w.root, "teardown-pwd.txt"), "utf8").trim(),
      realpathSync(w.root),
    );
  } finally {
    w.cleanup();
    rmSync(base, { recursive: true, force: true });
  }
});

test("a failing teardown and a timed-out teardown are logged and the worktree is still removed", async () => {
  for (const outcome of [
    { status: "failed", exitCode: 7, output: "boom\u001b[0m" },
    { status: "timeout" },
  ] as const) {
    const w = await world(
      true,
      true,
      3,
      {},
      { worktree: TEARDOWN, runTeardown: async () => outcome },
    );
    try {
      await launched(w);
      const first = await w.launcher.spawn("developer");
      const released = await w.launcher.release(first.agentId);
      assert.equal(released.worktreeRemoved, true);
      assert.deepEqual(w.git.removed, [first.worktreePath]);
      const failed = w.events.filter((e) => e.event === "teardown_failed");
      assert.equal(failed.length, 1);
      assert.equal(failed[0]!.details.agentId, "developer-1");
      assert.equal(
        failed[0]!.details.exit,
        outcome.status === "timeout" ? "timeout" : 7,
      );
      if (outcome.status === "failed")
        assert.equal(failed[0]!.details.output, "boom");
    } finally {
      w.cleanup();
    }
  }
});

test("a teardown that throws is logged and the worktree is still removed", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        throw new Error("spawn failed");
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    assert.equal(
      w.events.filter((e) => e.event === "teardown_failed").length,
      1,
    );
  } finally {
    w.cleanup();
  }
});

test("the real teardown runner kills the process group on timeout, logs it and still removes the worktree", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: {
        setupTimeoutSeconds: 600,
        teardown: "sleep 30 & echo $! > child.pid; wait",
        teardownTimeoutSeconds: 1,
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    const failed = w.events.filter((e) => e.event === "teardown_failed");
    assert.equal(failed.length, 1);
    assert.equal(failed[0]!.details.exit, "timeout");
    const pid = Number(readFileSync(path.join(w.root, "child.pid"), "utf8"));
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.throws(() => process.kill(pid, 0), /ESRCH/);
  } finally {
    w.cleanup();
  }
});

test("teardown is not run while the pane is still open, and runs on the retry once it closes", async () => {
  let runs = 0;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        runs += 1;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    await w.launcher.release(first.agentId);
    assert.equal(runs, 0);
    assert.deepEqual(w.git.removed, []);
    w.adapter.closeError = undefined;
    await w.launcher.spawn("developer");
    assert.equal(runs, 1);
    assert.deepEqual(w.git.removed, [first.worktreePath]);
  } finally {
    w.cleanup();
  }
});

test("teardown runs again when a refused removal is retried", async () => {
  let runs = 0;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        runs += 1;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.git.removeOk = false;
    await w.launcher.release(first.agentId);
    assert.equal(runs, 1);
    w.git.removeOk = true;
    await w.launcher.spawn("developer");
    assert.equal(runs, 2);
  } finally {
    w.cleanup();
  }
});

test("teardown is not run when no worktree path exists", async () => {
  let runs = 0;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        runs += 1;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    w.adapter.worktreeError = new HerdrError("worktree_failed", "no");
    await assert.rejects(w.launcher.spawn("developer"));
    assert.equal(runs, 0);
    assert.deepEqual(w.git.removed, []);
  } finally {
    w.cleanup();
  }
});

test("a failed setup, a replace and a config without teardown: teardown runs for the first two and never for the last", async () => {
  let runs = 0;
  const runTeardown: TeardownRunner = async () => {
    runs += 1;
    return { status: "ok" };
  };
  const failing = await world(
    true,
    true,
    3,
    {},
    {
      worktree: { ...TEARDOWN, setup: "npm ci" },
      runSetup: async () => ({ status: "timeout" }),
      runTeardown,
    },
  );
  try {
    await launched(failing);
    await assert.rejects(failing.launcher.spawn("developer"));
    assert.equal(runs, 1, "the failed spawn's worktree is torn down");
  } finally {
    failing.cleanup();
  }
  const replaced = await world(
    true,
    true,
    3,
    {},
    { worktree: TEARDOWN, runTeardown },
  );
  try {
    await launched(replaced);
    const old = await replaced.launcher.spawn("developer");
    await replaced.launcher.replace(old.agentId);
    assert.equal(runs, 2, "the replaced agent's worktree is torn down");
  } finally {
    replaced.cleanup();
  }
  runs = 0;
  const plain = await world(
    true,
    true,
    3,
    {},
    { worktree: SETUP, runSetup: async () => ({ status: "ok" }), runTeardown },
  );
  try {
    await launched(plain);
    const first = await plain.launcher.spawn("developer");
    await plain.launcher.release(first.agentId);
    assert.equal(runs, 0);
    assert.deepEqual(
      eventNames(plain).filter((name) => name.startsWith("teardown")),
      [],
    );
    assert.deepEqual(plain.git.removed, [first.worktreePath]);
  } finally {
    plain.cleanup();
  }
});

test("the documented codebase-memory teardown removes exactly the index files of its own worktree", () => {
  const reference = readFileSync("docs/reference/configuration.md", "utf8");
  const match = /^teardown = '(.+)'$/m.exec(reference);
  assert.ok(
    match !== null,
    "the configuration reference has a one-line teardown example",
  );
  const command = match[1]!;
  const home = mkdtempSync(path.join(tmpdir(), "capstan-teardown-home-"));
  try {
    const dir = path.join(home, ".cache", "codebase-memory-mcp");
    mkdirSync(dir, { recursive: true });
    const worktree = "/home/x/.herdr/worktrees/proj/proj-developer-3-g1";
    const own = "home-x-.herdr-worktrees-proj-proj-developer-3-g1";
    const ownFiles = [
      `${own}.db`,
      `${own}.db-shm`,
      `${own}.db-wal`,
      `${own}.db.stage.AbC123`,
      `${own}.db.stage.AbC123.lock`,
    ];
    const others = [
      "home-x-.herdr-worktrees-proj-proj-developer-33-g1.db",
      "home-x-.herdr-worktrees-proj-proj-developer-3-g10.db",
      "home-x-.herdr-worktrees-proj-proj-developer-3-g1-extra.db",
      "home-x-.herdr-worktrees-proj-proj-developer-4-g1.db-wal",
      "home-x-Workspace-proj.db",
      "_config.db",
    ];
    for (const name of [...ownFiles, ...others])
      writeFileSync(path.join(dir, name), "x");
    const run = (id: string) =>
      spawnSync("sh", ["-c", command], {
        env: {
          PATH: "/usr/bin:/bin",
          HOME: home,
          CAPSTAN_WORKTREE_PATH: worktree,
          CAPSTAN_AGENT_ID: id,
        },
        encoding: "utf8",
      });
    assert.equal(run("developer-3").status, 0);
    assert.deepEqual(readdirSync(dir).sort(), [...others].sort());
    assert.equal(run("developer-3").status, 0, "idempotent when run again");
    const empty = spawnSync("sh", ["-c", command], {
      env: { PATH: "/usr/bin:/bin", HOME: home },
    });
    assert.notEqual(empty.status, 0, "no worktree path removes nothing");
    assert.deepEqual(readdirSync(dir).sort(), [...others].sort());
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("teardown time is not charged to the cleanup budget", async () => {
  let clock = 1_000_000;
  const w = await world();
  try {
    await launched(w);
    const slow = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: { ...config(), worktree: TEARDOWN } as CapstanConfig,
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, ".capstan", "state", "control.sock"),
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: { PATH: "/usr/bin:/bin" },
      git: w.git,
      now: () => clock,
      runTeardown: async () => {
        clock += 5 * 60_000;
        return { status: "ok" };
      },
    });
    const first = await slow.spawn("developer");
    const released = await slow.release(first.agentId);
    assert.deepEqual(
      [released.paneClosed, released.worktreeRemoved, released.branchKept],
      [true, true, false],
    );
  } finally {
    w.cleanup();
  }
});

test("replace continues the predecessor's branch at its accepted commit, saves unreported commits at refs/capstan/kept and names them in the seed and the event", async () => {
  const w = await world();
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    const commit = "c".repeat(40);
    const unreported = "d".repeat(40);
    w.git.reachable.add(commit);
    w.git.tips.set(old.branch, unreported);
    reportAs(w, old.agentId, 1, commit, "parser written");
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(result.branch, old.branch, "the same branch name");
    assert.equal(result.keptRef, `refs/capstan/kept/${old.agentId}`);
    assert.equal(
      w.git.refs.get(`refs/capstan/kept/${old.agentId}`),
      unreported,
    );
    assert.ok(
      w.adapter.calls.includes(`worktree:${old.branch}:${commit}`),
      "the successor's worktree starts at the accepted commit",
    );
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === result.agentId)!
        .branch,
      old.branch,
    );
    const prompt = promptOf(w, 2);
    assert.ok(prompt.includes(`refs/capstan/kept/${old.agentId}`), prompt);
    assert.ok(prompt.includes(`(tip ${unreported})`));
    // Releasing the predecessor's leftovers later never touches the successor's branch.
    assert.equal(
      w.core.activeBranchHolder(old.branch, old.agentId),
      result.agentId,
    );
  } finally {
    w.cleanup();
  }
});

test("an ended agent's pane id that Herdr gave to a newer agent's pane is never closed by spawn, release or a restart", async () => {
  const w = await world();
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    // The worktree removal is refused, so the ended agent's row stays and every later operation looks at it again.
    w.git.removeOk = false;
    w.git.removeStderr = "fatal: worktree contains modified files";
    await w.launcher.release(old.agentId);
    assert.ok(
      w.core.agentPanes(w.owner).some((r) => r.agentId === old.agentId),
    );
    const project = w.adapter.metadata.find(
      (m) => m.tokens.agent === old.agentId,
    )!.tokens.project!;
    // Herdr reused the old short pane id for a newer agent's pane.
    w.adapter.identities.set(old.paneId, {
      terminalId: "term_newer",
      agent: "developer-9",
      project,
    });
    w.adapter.entries.set(old.paneId, { agent: "developer-9" });
    const closesOfOld = () =>
      w.adapter.calls.filter((c) => c === `close:${old.paneId}`).length;
    const before = closesOfOld();
    const next = await w.launcher.spawn("developer");
    await w.launcher.release(next.agentId);
    await w.reopen().adoptAll();
    assert.equal(closesOfOld(), before, "the reused pane id is never closed");
    assert.ok(
      w.adapter.calls.includes(`close:${next.paneId}`),
      "the released agent's own pane is closed",
    );
    assert.deepEqual(
      w.adapter.entries.get(old.paneId),
      { agent: "developer-9" },
      "the newer agent's registration is kept",
    );
    assert.ok(
      w.events.some(
        (e) =>
          e.event === "pane_not_owned" &&
          e.details?.agentId === old.agentId &&
          e.details?.paneId === old.paneId,
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("a pane that matches neither the recorded terminal id nor, on a row without one, both tokens is left open", async () => {
  const w = await world();
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    const project = w.adapter.metadata.find(
      (m) => m.tokens.agent === old.agentId,
    )!.tokens.project!;
    // Same agent token, but another terminal: a pane id reused by a pane that copied the token is still not this agent's.
    w.adapter.identities.set(old.paneId, {
      terminalId: "term_other",
      agent: old.agentId,
      project,
    });
    const released = await w.launcher.release(old.agentId);
    assert.equal(released.paneClosed, true, "the agent's own pane is gone");
    assert.equal(w.adapter.calls.includes(`close:${old.paneId}`), false);
  } finally {
    w.cleanup();
  }
});

test("a worktree whose directory is gone, or that git says is not a working tree, counts as removed and its teardown never runs again", async () => {
  let runs = 0;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        runs += 1;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    // An agent whose removal was refused and whose directory was then deleted by hand: the row of the reported bug.
    const gone = await w.launcher.spawn("developer");
    w.git.removeOk = false;
    w.git.removeStderr = "fatal: worktree contains modified files";
    await w.launcher.release(gone.agentId);
    assert.equal(runs, 1);
    w.git.missing.add(gone.worktreePath);
    // An agent whose worktree git no longer knows.
    const unknown = await w.launcher.spawn("developer");
    assert.equal(runs, 1, "a missing worktree has nothing to tear down");
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === gone.agentId),
      false,
      "the finished cleanup's row is gone",
    );
    w.git.removeStderr = `fatal: '${unknown.worktreePath}' is not a working tree`;
    const released = await w.launcher.release(unknown.agentId);
    assert.equal(released.worktreeRemoved, true);
    assert.equal(runs, 2);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === unknown.agentId),
      false,
    );
    assert.ok(w.git.prunes >= 2);
    const teardownsBefore = w.events.filter(
      (e) => e.event === "teardown_started",
    ).length;
    const removalsBefore = w.git.removed.length;
    const identifiedBefore = w.adapter.identified.length;
    w.git.removeOk = true;
    const later = await w.launcher.spawn("developer");
    await w.reopen().adoptAll();
    assert.equal(
      w.events.filter((e) => e.event === "teardown_started").length,
      teardownsBefore,
      "no finished teardown is started again",
    );
    assert.equal(w.git.removed.length, removalsBefore);
    assert.deepEqual(
      w.adapter.identified
        .slice(identifiedBefore)
        .filter((p) => p === gone.paneId || p === unknown.paneId),
      [],
      "no pane of a finished cleanup is looked at, let alone closed",
    );
    assert.deepEqual(w.launcher.status().cleanupFailed, []);
    await w.launcher.release(later.agentId);
  } finally {
    w.cleanup();
  }
});

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { defaultGit } from "../src/launcher.js";
import { LOSS_LIMIT, SUPPRESS_LIMIT } from "../src/driver.js";
import { HerdrError } from "../src/herdr/runner.js";
import { ctx } from "./harness.js";
import { recoveryWorld, type RecoveryWorld } from "./recovery-harness.js";

async function withWorld(
  run: (w: RecoveryWorld) => Promise<void>,
): Promise<void> {
  const w = await recoveryWorld();
  try {
    await run(w);
  } finally {
    w.close();
  }
}

const lostEvents = (w: RecoveryWorld, agentId: string): number =>
  w.lostRows(agentId);

test("a crash between the record and the typing loses the delivery visibly and never types it again; a recorded retry sends it once more", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    const first = w.send(dev, "first instruction");
    const second = w.send(dev, "second instruction");
    w.world.crashAt = "after_record_before_typing";
    await w.tick();
    await w.crash();
    assert.equal(w.state(first), "sent", "the record happened");
    assert.equal(w.world.deliveries(first), 0, "nothing was typed");
    await w.tick(4);
    assert.equal(
      w.world.deliveries(first),
      0,
      "the driver never types it again",
    );
    assert.equal(w.state(first), "sent");
    assert.equal(
      w.world.deliveries(second),
      0,
      "the next waits behind the unacked one",
    );
    w.advance(601_000);
    await w.tick();
    assert.equal(w.state(first), "unacked", "the loss is visible");
    w.core.resolveMessage(ctx(w.core, w.owner), first, "retry");
    await w.tick();
    assert.equal(w.world.deliveries(first), 1);
    assert.equal(w.retries(first), 1);
    assert.equal(w.sentEvents(first), 2, "recorded as sent twice, typed once");
    w.assertNoDuplicate(first);
    w.ack(first);
    await w.tick();
    assert.equal(w.world.deliveries(second), 1);
    w.assertNoDuplicate(second);
    assert.equal(w.world.typedSends, 2);
  });
});

test("a crash after the typing does not type again, the ack still works after the restart and the count stays one", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    const first = w.send(dev, "first instruction");
    w.world.crashAt = "after_typing";
    await w.tick();
    await w.crash();
    assert.equal(w.world.deliveries(first), 1);
    assert.equal(w.state(first), "sent");
    await w.tick(5);
    assert.equal(w.world.deliveries(first), 1, "no retype after the restart");
    w.ack(first);
    await w.tick(3);
    assert.equal(w.state(first), "acked");
    assert.equal(w.world.deliveries(first), 1);
    assert.equal(w.retries(first), 0);
    w.assertNoDuplicate(first);
  });
});

test("overlapping ticks and a restart between ticks never deliver an instruction twice", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    const ids = [1, 2, 3].map((n) => w.send(dev, `instruction ${n}`));
    await Promise.all([w.tick(), w.tick(), w.tick()]);
    assert.equal(w.world.deliveries(ids[0]!), 1);
    await w.crash();
    await Promise.all([w.tick(), w.tick()]);
    w.ack(ids[0]!);
    await w.tick();
    await w.crash();
    w.ack(ids[1]!);
    await w.tick(3);
    w.ack(ids[2]!);
    await w.tick();
    for (const id of ids) {
      assert.equal(w.world.deliveries(id), 1, id);
      w.assertNoDuplicate(id);
    }
  });
});

test("a dead pane is recorded lost after three not-found observations, once, with one notice to the PM, also across a restart", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    w.world.kill(dev);
    await w.tick(LOSS_LIMIT - 1);
    assert.deepEqual(w.driver.snapshot().lostAgentIds, []);
    assert.equal(lostEvents(w, dev), 0);
    await w.tick();
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [dev]);
    const notices = w.core.messagesFor(w.pm.agentId);
    assert.equal(notices.length, 1);
    assert.match(
      notices[0]!.body,
      new RegExp(
        `^Agent ${dev} \\(role developer, Developer\\) is lost: Herdr no longer finds its pane or process\\n`,
      ),
    );
    assert.match(notices[0]!.body, /cstan replace developer-agent/);
    assert.equal(
      w.core.agentRecord(dev)!.state,
      "active",
      "the controller does not end or replace it",
    );
    await w.tick(5);
    assert.equal(lostEvents(w, dev), 1);
    await w.crash();
    await w.tick(LOSS_LIMIT + 2);
    assert.equal(lostEvents(w, dev), 1, "a restart cannot report it twice");
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [dev]);
    assert.ok(w.events.some((e) => e.event === "agent_lost"));
  });
});

test("one successful observation resets the count, and an agent that is seen again is no longer listed lost", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    w.world.kill(dev);
    await w.tick(LOSS_LIMIT - 1);
    w.world.pane(dev).alive = true;
    await w.tick();
    w.world.kill(dev);
    await w.tick(LOSS_LIMIT - 1);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [],
      "the count started again",
    );
    await w.tick();
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [dev]);
    w.world.pane(dev).alive = true;
    await w.tick();
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [],
      "listed only while the driver sees it failing",
    );
    assert.equal(lostEvents(w, dev), 1, "the ledger keeps the event");
  });
});

test("only agent and pane not-found count: a server that does not answer, a missing session, workspace or tab and any other error never mark an agent lost", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    for (const error of [
      new HerdrError("server_unreachable", "no answer"),
      new HerdrError("session_not_found", "no session"),
      new HerdrError("workspace_not_found", "no workspace"),
      new HerdrError("tab_not_found", "no tab"),
      new Error("boom"),
    ]) {
      w.world.pane(dev).observeError = error;
      await w.tick(LOSS_LIMIT + 2);
      assert.deepEqual(w.driver.snapshot().lostAgentIds, [], String(error));
    }
    assert.equal(lostEvents(w, dev), 0);
    w.world.pane(dev).observeError = new HerdrError(
      "pane_not_found",
      "pane gone",
    );
    await w.tick(LOSS_LIMIT);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [dev],
      "pane_not_found counts",
    );
  });
});

test("when every observed agent is not found in the same tick nothing counts (a Herdr restart), until the hold runs out", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    w.world.kill(dev);
    w.world.kill(w.pm.agentId);
    await w.tick(SUPPRESS_LIMIT - 1);
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [], "held");
    assert.ok(w.events.some((e) => e.event === "loss_suppressed"));
    w.world.pane(w.pm.agentId).alive = true;
    await w.tick(LOSS_LIMIT - 1);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [],
      "held counters were not counted, so the count starts now",
    );
    await w.tick();
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [dev]);
  });
  await withWorld(async (w) => {
    w.world.kill(w.developer.agentId);
    w.world.kill(w.pm.agentId);
    await w.tick(SUPPRESS_LIMIT + 1);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [],
      "the hold ran out and the counters are zero",
    );
    await w.tick(LOSS_LIMIT);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [w.developer.agentId, w.pm.agentId].sort(),
      "panes that really closed together are reported after three more failures",
    );
  });
});

test("a failed delivery to a dead pane stays failed when the agent is released; retry is refused for the ended recipient and a cancel keeps the failure", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    const id = w.send(dev, "do the thing");
    w.world.kill(dev);
    await w.tick(6);
    assert.equal(w.state(id), "failed");
    const reason = w.core.message(id)!.stateReason;
    assert.ok(reason, "the failure has a reason");
    w.core.endAgent(ctx(w.core, w.owner), dev);
    assert.equal(
      w.state(id),
      "failed",
      "ending the agent does not overwrite the failure",
    );
    assert.equal(w.core.message(id)!.stateReason, reason);
    assert.throws(
      () => w.core.resolveMessage(ctx(w.core, w.owner), id, "retry"),
      /recipient agent has ended|recipient_not_active/,
    );
    assert.equal(w.retries(id), 0, "a refused retry writes no decision");
    w.core.resolveMessage(ctx(w.core, w.owner), id, "cancel");
    assert.equal(w.state(id), "cancelled");
    assert.equal(
      w.core.message(id)!.stateReason,
      `resolution_cancel (was failed: ${reason})`,
    );
    assert.equal(w.world.deliveries(id), 0);
  });
});

test("an agent found dead at a daemon start is ended, recorded lost and reported to the PM in one step", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    const queued = w.send(dev, "not done");
    const ended = w.core.endAgent(ctx(w.core, w.owner), dev, {
      lost: "found_dead_at_start",
    });
    assert.deepEqual(ended.cancelledMessageIds, [queued]);
    const notices = w.core.messagesFor(w.pm.agentId);
    assert.equal(notices.length, 1);
    assert.match(
      notices[0]!.body,
      /its pane was gone when the daemon started, so the controller ended it/,
    );
    assert.ok(
      notices[0]!.body.includes(queued),
      "the not-done message is named",
    );
    assert.equal(w.core.agentRecord(dev)!.state, "ended");
    assert.equal(
      w.lostRows(dev),
      1,
      "the loss is in the ledger, in the same step as the end",
    );
  });
});

function git(root: string, ...args: string[]): string {
  return execFileSync("git", ["-C", root, ...args], {
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_AUTHOR_NAME: "t",
      GIT_AUTHOR_EMAIL: "t@e.c",
      GIT_COMMITTER_NAME: "t",
      GIT_COMMITTER_EMAIL: "t@e.c",
    },
  }).trim();
}

test("the launcher's git helpers read a branch tip and accept only a commit that exists and lies under one of the given refs", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-seedgit-"));
  try {
    git(root, "init", "-q", "-b", "main");
    const commit = (name: string): string => {
      writeFileSync(path.join(root, name), name);
      git(root, "add", name);
      git(root, "commit", "-q", "-m", name);
      return git(root, "rev-parse", "HEAD");
    };
    const base = commit("base");
    git(root, "checkout", "-q", "-b", "capstan/worker-1-g1");
    const onBranch = commit("work");
    git(root, "checkout", "-q", "main");
    const onMain = commit("later");
    const tool = defaultGit(root);
    assert.equal(tool.branchTip("capstan/worker-1-g1"), onBranch);
    assert.equal(tool.branchTip("capstan/nobody"), null);
    assert.equal(tool.branchTip("bad..name"), null);
    assert.equal(
      tool.reachableCommit(onBranch, ["refs/heads/capstan/worker-1-g1"]),
      true,
    );
    assert.equal(
      tool.reachableCommit(onBranch, ["HEAD"]),
      false,
      "main does not hold it",
    );
    assert.equal(
      tool.reachableCommit(onBranch, [
        "HEAD",
        "refs/heads/capstan/worker-1-g1",
      ]),
      true,
    );
    assert.equal(tool.reachableCommit(base, ["HEAD"]), true);
    assert.equal(
      tool.reachableCommit(onMain, ["refs/heads/capstan/missing"]),
      false,
      "a missing ref is not an error",
    );
    assert.equal(
      tool.reachableCommit("f".repeat(40), ["HEAD"]),
      false,
      "an unknown commit",
    );
    assert.equal(
      tool.reachableCommit(git(root, "rev-parse", "HEAD^{tree}"), ["HEAD"]),
      false,
      "a tree is not a commit",
    );
    assert.equal(tool.reachableCommit("not-a-sha", ["HEAD"]), false);
    assert.equal(
      tool.reachableCommit("A".repeat(40), ["HEAD"]),
      false,
      "uppercase is not accepted",
    );
    assert.equal(tool.reachableCommit("--output=x", ["HEAD"]), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a loss the ledger could not take is not listed and is tried again on the next tick", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    const real = w.core.recordAgentLost.bind(w.core);
    let failures = 1;
    w.core.recordAgentLost = (context, input) => {
      if (failures > 0) {
        failures -= 1;
        throw new Error("database busy");
      }
      return real(context, input);
    };
    w.world.kill(dev);
    await w.tick(LOSS_LIMIT);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [],
      "not listed until recorded",
    );
    assert.equal(w.lostRows(dev), 0);
    assert.ok(w.events.some((e) => e.event === "agent_lost_not_recorded"));
    await w.tick();
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [dev]);
    assert.equal(w.lostRows(dev), 1);
  });
});

test("a count left from before a pane disappeared does not carry over when it comes back", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    w.world.kill(dev);
    await w.tick(LOSS_LIMIT - 1);
    const pane = w.world.paneForAgent(dev)!;
    w.world.unregister(dev);
    await w.tick(3);
    w.world.register(dev, pane);
    await w.tick(1);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [],
      "one failure after the pane came back is only the first",
    );
    await w.tick(LOSS_LIMIT - 1);
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [dev]);
  });
});

test("an observation that fails for another reason breaks the row of not-found observations", async () => {
  await withWorld(async (w) => {
    const dev = w.developer.agentId;
    w.world.kill(dev);
    await w.tick(LOSS_LIMIT - 1);
    w.world.pane(dev).alive = true;
    w.world.pane(dev).observeError = new HerdrError(
      "timeout",
      "no answer in time",
    );
    await w.tick();
    w.world.pane(dev).observeError = undefined;
    w.world.kill(dev);
    await w.tick(LOSS_LIMIT - 1);
    assert.deepEqual(
      w.driver.snapshot().lostAgentIds,
      [],
      "the other error reset the count",
    );
    await w.tick();
    assert.deepEqual(w.driver.snapshot().lostAgentIds, [dev]);
  });
});

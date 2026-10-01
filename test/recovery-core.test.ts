import assert from "node:assert/strict";
import { test } from "node:test";
import type { ReportEvidence } from "../src/controller/core.js";
import { SEED_MESSAGES, SEED_REPORTS } from "../src/controller/core.js";
import { close, ctx, harness, type Harness, type Member } from "./harness.js";

const BASE = "a".repeat(40);

function worker(h: Harness, name: string): Member {
  return h.addMember(name, "Developer");
}

/** Another agent of the role `developer` (addMember gives each agent a role of its own name). */
function sameRole(h: Harness, name: string): Member {
  const seatId = `${name}-seat`;
  h.core.createSeat(ctx(h.core, h.owner), { seatId, name, role: "Developer" });
  const actor = h.core.createActor(ctx(h.core, h.owner), {
    displayName: name,
    role: "Developer",
    seatId,
  });
  h.core.registerAgent(ctx(h.core, h.owner), {
    agentId: name,
    roleName: "developer",
    seatId,
    actorId: actor.actorId,
  });
  return {
    agentId: name,
    credential: actor.credential,
    actorId: actor.actorId,
  };
}

function accepted(
  h: Harness,
  author: Member,
  commit: string,
  summary = "did it",
): void {
  const branch = `capstan/${author.agentId}-g1`;
  h.core.recordAgentPane(ctx(h.core, h.owner), {
    agentId: author.agentId,
    workspaceId: null,
    paneId: null,
    worktreePath: null,
    branch,
    baseSha: BASE,
  });
  const evidence: ReportEvidence = {
    generation: 1,
    branch,
    baseSha: BASE,
    commitExists: true,
    branchTip: commit,
    isAncestorOfTip: true,
    isAncestorOfBase: false,
    checkedAt: "2026-10-01T00:00:00.000Z",
  };
  const { record } = h.core.recordAgentReport(ctx(h.core, author.credential), {
    commitSha: commit,
    summary,
    evidence,
  });
  assert.equal(record.state, "accepted");
}

test("recordAgentLost writes one event per agent and generation, tells the PM once and leaves the agent active", async () => {
  const h = await harness();
  try {
    const other = worker(h, "developer2");
    const queued = h.core.enqueueMessage(ctx(h.core, h.pm.credential), {
      recipientAgentId: h.developer.agentId,
      body: "not done",
    }).messageId;
    const first = h.core.recordAgentLost(ctx(h.core, h.owner), {
      agentId: h.developer.agentId,
    });
    assert.deepEqual(first, { recorded: true });
    const again = h.core.recordAgentLost(ctx(h.core, h.owner), {
      agentId: h.developer.agentId,
    });
    assert.deepEqual(again, { recorded: false });
    const notices = h.core
      .messagesFor(h.pm.agentId)
      .filter((m) => m.body.startsWith(`Agent ${h.developer.agentId} `));
    assert.equal(notices.length, 1);
    assert.ok(notices[0]!.body.includes(queued));
    assert.equal(h.core.senderOf(notices[0]!.senderActorId).role, "controller");
    assert.equal(h.core.agentRecord(h.developer.agentId)!.state, "active");
    assert.equal(
      h.core.message(queued)!.state,
      "queued",
      "nothing is cancelled",
    );
    h.core.recordAgentLost(ctx(h.core, h.owner), { agentId: other.agentId });
    assert.equal(h.core.messagesFor(h.pm.agentId).length, 2);
    h.core.endAgent(ctx(h.core, h.owner), h.developer.agentId);
    assert.throws(
      () =>
        h.core.recordAgentLost(ctx(h.core, h.owner), {
          agentId: h.developer.agentId,
        }),
      /not active/,
    );
    assert.throws(
      () => h.core.recordAgentLost(ctx(h.core, h.owner), { agentId: "nobody" }),
      /not active/,
    );
    assert.throws(
      () =>
        h.core.recordAgentLost(ctx(h.core, other.credential), {
          agentId: other.agentId,
        }),
      /capability|Authorization|not allowed|does not have/i,
    );
  } finally {
    await close(h);
  }
});

test("a lost PM is recorded but not reported to itself, and with no PM active the event is the record", async () => {
  const h = await harness();
  try {
    h.core.recordAgentLost(ctx(h.core, h.owner), { agentId: h.pm.agentId });
    assert.equal(h.core.messagesFor(h.pm.agentId).length, 0);
    h.core.endAgent(ctx(h.core, h.owner), h.pm.agentId);
    h.core.recordAgentLost(ctx(h.core, h.owner), {
      agentId: h.developer.agentId,
    });
    assert.equal(h.core.messagesFor(h.pm.agentId).length, 0, "no PM to tell");
    assert.deepEqual(
      h.core.recordAgentLost(ctx(h.core, h.owner), {
        agentId: h.developer.agentId,
      }),
      { recorded: false },
    );
  } finally {
    await close(h);
  }
});

test("a replacement is recorded once, for another active agent of the same role, never for a PM", async () => {
  const h = await harness();
  try {
    const successor = sameRole(h, "developer-two");
    const otherRole = worker(h, "developer2");
    const record = () =>
      h.core.recordAgentReplaced(ctx(h.core, h.owner), {
        predecessorId: h.developer.agentId,
        successorId: successor.agentId,
      });
    assert.equal(h.core.isAgentReplaced(h.developer.agentId), false);
    assert.throws(
      () =>
        h.core.recordAgentReplaced(ctx(h.core, h.owner), {
          predecessorId: h.pm.agentId,
          successorId: successor.agentId,
        }),
      /PM is restarted/,
    );
    assert.throws(
      () =>
        h.core.recordAgentReplaced(ctx(h.core, h.owner), {
          predecessorId: h.developer.agentId,
          successorId: h.developer.agentId,
        }),
      /another active agent of the same role/,
    );
    assert.throws(
      () =>
        h.core.recordAgentReplaced(ctx(h.core, h.owner), {
          predecessorId: h.developer.agentId,
          successorId: otherRole.agentId,
        }),
      /same role/,
    );
    assert.throws(
      () =>
        h.core.recordAgentReplaced(ctx(h.core, h.owner), {
          predecessorId: "nobody",
          successorId: successor.agentId,
        }),
      /must exist/,
    );
    assert.deepEqual(record(), { recorded: true });
    assert.equal(h.core.isAgentReplaced(h.developer.agentId), true);
    assert.throws(record, /already replaced/);
    h.core.endAgent(ctx(h.core, h.owner), successor.agentId);
    const ended = sameRole(h, "dev-three");
    const fresh = sameRole(h, "dev-four");
    h.core.endAgent(ctx(h.core, h.owner), ended.agentId);
    assert.throws(
      () =>
        h.core.recordAgentReplaced(ctx(h.core, h.owner), {
          predecessorId: fresh.agentId,
          successorId: ended.agentId,
        }),
      /another active agent of the same role/,
      "a successor that has ended is refused",
    );
  } finally {
    await close(h);
  }
});

test("the seed reads the newest messages, the accepted reports and the open findings of any agent, ended ones included, in ledger order", async () => {
  const h = await harness();
  try {
    const dev = h.developer;
    const ids: string[] = [];
    for (let i = 0; i < SEED_MESSAGES + 3; i += 1)
      ids.push(
        h.core.enqueueMessage(ctx(h.core, h.pm.credential), {
          recipientAgentId: dev.agentId,
          body: `instruction ${i}`,
        }).messageId,
      );
    for (let i = 0; i < SEED_REPORTS + 2; i += 1)
      accepted(h, dev, String(i).padStart(2, "0").repeat(20), `step ${i}`);
    const seed = h.core.agentSeed(dev.agentId);
    assert.equal(seed.agentId, dev.agentId);
    assert.equal(seed.roleName, "developer");
    assert.equal(seed.messages.length, SEED_MESSAGES);
    assert.equal(seed.messagesOmitted, 3);
    assert.deepEqual(
      seed.messages.map((m) => m.messageId),
      ids.slice(3),
      "newest 20, oldest first",
    );
    assert.equal(seed.messages[0]!.sender, h.pm.agentId);
    assert.equal(seed.reports.length, SEED_REPORTS);
    assert.equal(seed.reportsOmitted, 2);
    assert.equal(seed.reports.at(-1)!.summary, `step ${SEED_REPORTS + 1}`);
    assert.equal(
      seed.lastAcceptedCommit,
      String(SEED_REPORTS + 1)
        .padStart(2, "0")
        .repeat(20),
    );
    assert.equal(seed.branch, `capstan/${dev.agentId}-g1`);
    assert.equal(seed.baseSha, BASE);
    h.core.endAgent(ctx(h.core, h.owner), dev.agentId);
    const ended = h.core.agentSeed(dev.agentId);
    assert.equal(ended.state, "ended");
    assert.equal(ended.reports.length, SEED_REPORTS);
    assert.throws(() => h.core.agentSeed("nobody"), /does not exist/);
  } finally {
    await close(h);
  }
});

test("ending an agent keeps its failed messages failed and cancels the rest; a PM generation replacement still cancels a failed one with the reason kept", async () => {
  const h = await harness();
  try {
    const dev = h.developer.agentId;
    const make = (): string =>
      h.core.enqueueMessage(ctx(h.core, h.pm.credential), {
        recipientAgentId: dev,
        body: "x",
      }).messageId;
    const failing = make();
    h.core.recordFailure(
      ctx(h.core, h.owner),
      failing,
      "Herdr failed: agent_not_found",
    );
    const waiting = make();
    const ended = h.core.endAgent(ctx(h.core, h.owner), dev);
    assert.deepEqual(ended.cancelledMessageIds, [waiting]);
    assert.equal(h.core.message(failing)!.state, "failed");
    assert.equal(
      h.core.message(failing)!.stateReason,
      "Herdr failed: agent_not_found",
    );
    assert.equal(h.core.message(waiting)!.stateReason, "agent_ended");
    const toPm = h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.pm.agentId,
      body: "for the pm",
    }).messageId;
    h.core.recordFailure(ctx(h.core, h.owner), toPm, "r".repeat(250));
    h.core.replaceAgentGeneration(ctx(h.core, h.owner), h.pm.agentId);
    const reason = h.core.message(toPm)!.stateReason!;
    assert.match(reason, /^generation_replaced \(was failed: r{199}…\)$/);
    assert.ok(!reason.includes("\n"));
  } finally {
    await close(h);
  }
});

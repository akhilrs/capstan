import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import Database from "better-sqlite3";
import { close, ctx, harness, type Harness } from "./harness.js";

const SHA = "a".repeat(40);

function pane(agentId: string, overrides: Record<string, string | null> = {}) {
  return {
    agentId,
    workspaceId: "w1",
    paneId: "w1:p1",
    worktreePath: null,
    branch: null,
    baseSha: null,
    ...overrides,
  };
}

function writableDatabase(h: Harness): Database.Database {
  return new Database(path.join(h.stateDirectory, "controller.sqlite"));
}

function addWork(h: Harness, id: string, state: string): void {
  h.core.createWorkItem(ctx(h.core, h.owner), {
    workItemId: id,
    title: `Work ${id}`,
    description: "d",
    requiredRole: "Developer",
  });
  const db = writableDatabase(h);
  try {
    db.prepare("UPDATE work_items SET state = ? WHERE work_item_id = ?").run(
      state,
      id,
    );
  } finally {
    db.close();
  }
}

test("pane rows are recorded, updated with the agent's generation, read back and cleared, for a controller only", async () => {
  const h = await harness();
  try {
    assert.deepEqual(h.core.agentPanes(h.owner), []);
    h.core.recordAgentPane(ctx(h.core, h.owner), pane(h.developer.agentId));
    h.core.recordAgentPane(
      ctx(h.core, h.owner),
      pane(h.developer.agentId, {
        paneId: "w2:p1",
        worktreePath: "/tmp/work/tree",
        branch: "capstan/developer-1",
        baseSha: SHA,
      }),
    );
    const rows = h.core.agentPanes(h.owner);
    assert.equal(rows.length, 1);
    assert.deepEqual(rows[0], {
      agentId: h.developer.agentId,
      workspaceId: "w1",
      paneId: "w2:p1",
      worktreePath: "/tmp/work/tree",
      branch: "capstan/developer-1",
      baseSha: SHA,
      generation: 1,
    });
    assert.throws(
      () => h.core.agentPanes(h.developer.credential),
      /capabilit|authoriz|permit/i,
    );
    assert.throws(
      () =>
        h.core.recordAgentPane(
          ctx(h.core, h.developer.credential),
          pane(h.developer.agentId),
        ),
      /capabilit|authoriz|permit/i,
    );
    assert.deepEqual(
      h.core.clearAgentPane(ctx(h.core, h.owner), h.developer.agentId),
      { cleared: true },
    );
    assert.deepEqual(
      h.core.clearAgentPane(ctx(h.core, h.owner), h.developer.agentId),
      { cleared: false },
    );
    assert.deepEqual(h.core.agentPanes(h.owner), []);
  } finally {
    await close(h);
  }
});

test("an intent row may hold no pane, and bad values and unknown or ended agents are refused", async () => {
  const h = await harness();
  try {
    h.core.recordAgentPane(
      ctx(h.core, h.owner),
      pane(h.developer.agentId, {
        workspaceId: null,
        paneId: null,
        branch: "capstan/developer-1",
        baseSha: SHA,
      }),
    );
    assert.equal(h.core.agentPanes(h.owner)[0]!.paneId, null);
    for (const bad of [
      { worktreePath: "relative/path" },
      { worktreePath: "/tmp/a\nb" },
      { branch: "-bad" },
      { branch: "a b" },
      { baseSha: "xyz" },
      { baseSha: "A".repeat(40) },
      { paneId: "bad id" },
    ])
      assert.throws(
        () =>
          h.core.recordAgentPane(
            ctx(h.core, h.owner),
            pane(h.developer.agentId, bad),
          ),
        TypeError,
        JSON.stringify(bad),
      );
    assert.throws(
      () => h.core.recordAgentPane(ctx(h.core, h.owner), pane("nobody")),
      /active agent/,
    );
    h.core.endAgent(ctx(h.core, h.owner), h.developer.agentId);
    assert.throws(
      () =>
        h.core.recordAgentPane(ctx(h.core, h.owner), pane(h.developer.agentId)),
      /active agent/,
    );
    assert.deepEqual(
      h.core.clearAgentPane(ctx(h.core, h.owner), h.developer.agentId),
      { cleared: true },
      "a pane row of an ended agent can be cleared",
    );
  } finally {
    await close(h);
  }
});

test("the fallback pane is one row, replaced and cleared", async () => {
  const h = await harness();
  try {
    assert.equal(h.core.fallbackPane(h.owner), undefined);
    h.core.recordFallbackPane(ctx(h.core, h.owner), {
      workspaceId: "w3",
      paneId: "w3:p1",
    });
    h.core.recordFallbackPane(ctx(h.core, h.owner), {
      workspaceId: "w4",
      paneId: "w4:p1",
    });
    assert.deepEqual(h.core.fallbackPane(h.owner), {
      workspaceId: "w4",
      paneId: "w4:p1",
    });
    assert.deepEqual(h.core.clearFallbackPane(ctx(h.core, h.owner)), {
      cleared: true,
    });
    assert.deepEqual(h.core.clearFallbackPane(ctx(h.core, h.owner)), {
      cleared: false,
    });
    assert.throws(
      () => h.core.fallbackPane(h.pm.credential),
      /capabilit|authoriz|permit/i,
    );
  } finally {
    await close(h);
  }
});

test("a restart summary lists open work and open messages, is recorded before the replace, and the replace still cancels the messages", async () => {
  const h = await harness();
  try {
    addWork(h, "w-pending", "pending");
    addWork(h, "w-running", "running");
    addWork(h, "w-blocked", "blocked");
    addWork(h, "w-accepted", "accepted");
    addWork(h, "w-canceled", "canceled");
    addWork(h, "w-failed", "failed");
    const queued = h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.pm.agentId,
      body: "queued for the PM",
    }).messageId;
    const sent = h.core.enqueueMessage(ctx(h.core, h.developer.credential), {
      recipientAgentId: h.pm.agentId,
      body: "worker report",
    }).messageId;
    h.core.pullMessage(ctx(h.core, h.pm.credential));
    const before = h.pm.credential;
    const result = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.equal(result.generation, 2);
    assert.notEqual(result.credential, before);
    assert.deepEqual(
      [...result.cancelledMessageIds].sort(),
      [queued, sent].sort(),
      "the replace cancelled them",
    );
    assert.deepEqual(
      result.summary.messages.map((m) => [m.messageId, m.state]).sort(),
      [
        [queued, "sent"],
        [sent, "queued"],
      ].sort(),
      "the summary saw them first",
    );
    const db = writableDatabase(h);
    let directQuery: string[];
    try {
      directQuery = (
        db
          .prepare(
            "SELECT work_item_id FROM work_items WHERE state NOT IN ('accepted','canceled','failed') ORDER BY created_at, work_item_id",
          )
          .all() as Array<{ work_item_id: string }>
      ).map((row) => row.work_item_id);
    } finally {
      db.close();
    }
    assert.deepEqual(
      result.summary.openWork.map((w) => w.workItemId),
      directQuery,
    );
    assert.deepEqual(directQuery.sort(), [
      "w-blocked",
      "w-pending",
      "w-running",
    ]);
    const rows = h.core.pmRestarts(h.owner, h.pm.agentId);
    assert.equal(rows.length, 1);
    assert.equal(rows[0]!.summarizedGeneration, 1);
    assert.equal(rows[0]!.consumed, false);
    assert.equal(rows[0]!.sequence, 1);
    assert.deepEqual(rows[0]!.summary.openWork, result.summary.openWork);
  } finally {
    await close(h);
  }
});

test("a restart carries unconsumed summaries forward and marks them consumed together", async () => {
  const h = await harness();
  try {
    const first = h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.pm.agentId,
      body: "lost in the first restart",
    }).messageId;
    h.core.restartAgentGeneration(ctx(h.core, h.owner), h.pm.agentId);
    const second = h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.pm.agentId,
      body: "lost in the second",
    }).messageId;
    h.core.restartAgentGeneration(ctx(h.core, h.owner), h.pm.agentId);
    const third = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.deepEqual(
      third.summary.messages.map((m) => m.messageId).sort(),
      [first, second].sort(),
      "both earlier rows are merged, deduplicated by id",
    );
    assert.deepEqual(
      h.core.markPmRestartsConsumed(ctx(h.core, h.owner), h.pm.agentId, 3),
      { marked: 3 },
    );
    assert.ok(
      h.core.pmRestarts(h.owner, h.pm.agentId).every((r) => r.consumed),
    );
    const fourth = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.deepEqual(
      fourth.summary.messages,
      [],
      "consumed rows are never replayed",
    );
    assert.throws(
      () =>
        h.core.markPmRestartsConsumed(ctx(h.core, h.owner), h.pm.agentId, 0),
      TypeError,
    );
  } finally {
    await close(h);
  }
});

test("a restart is refused for a worker or an ended agent and leaves no attempt row; summaries are immutable and bounded", async () => {
  const h = await harness();
  try {
    assert.throws(
      () =>
        h.core.restartAgentGeneration(
          ctx(h.core, h.owner),
          h.developer.agentId,
        ),
      /only a PM/,
    );
    assert.throws(
      () =>
        h.core.restartAgentGeneration(
          ctx(h.core, h.developer.credential),
          h.pm.agentId,
        ),
      /capabilit|authoriz|permit/i,
    );
    assert.deepEqual(h.core.pmRestarts(h.owner, h.pm.agentId), []);

    for (let i = 0; i < 60; i += 1)
      h.core.enqueueMessage(ctx(h.core, h.owner), {
        recipientAgentId: h.pm.agentId,
        body: i === 0 ? "é".repeat(3000) : `message ${i}`,
      });
    const big = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.equal(big.summary.messages.length, 50);
    assert.equal(big.summary.truncated, true);
    assert.ok(big.summary.messages[0]!.body.endsWith("[truncated]"));

    const db = writableDatabase(h);
    try {
      assert.throws(
        () => db.prepare("UPDATE pm_restarts SET summary_json = '{}'").run(),
        /immutable/,
      );
      assert.throws(
        () => db.prepare("DELETE FROM pm_restarts").run(),
        /immutable/,
      );
      assert.equal(
        db.prepare("UPDATE pm_restarts SET consumed = 1").run().changes,
        1,
      );
    } finally {
      db.close();
    }
  } finally {
    await close(h);
  }
});

test("the new credential of a restart is never stored in plain text", async () => {
  const h = await harness();
  try {
    const result = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    const db = writableDatabase(h);
    try {
      const rows = db
        .prepare("SELECT result_json FROM mutation_requests")
        .all() as Array<{
        result_json: string;
      }>;
      assert.ok(rows.length > 0);
      for (const row of rows)
        assert.ok(!row.result_json.includes(result.credential));
      const events = db
        .prepare(
          "SELECT payload_json FROM controller_events WHERE payload_json LIKE '%summaryHash%'",
        )
        .all() as Array<{ payload_json: string }>;
      assert.ok(events.length >= 1);
      for (const event of events)
        assert.ok(!event.payload_json.includes("queued for the PM"));
    } finally {
      db.close();
    }
  } finally {
    await close(h);
  }
});

test("seatActorIds lists the active actors of a seat", async () => {
  const h = await harness();
  try {
    const ids = h.core.seatActorIds(h.owner, "pm-seat");
    assert.deepEqual(ids, [h.pm.actorId]);
    h.core.restartAgentGeneration(ctx(h.core, h.owner), h.pm.agentId);
    const after = h.core.seatActorIds(h.owner, "pm-seat");
    assert.equal(after.length, 1);
    assert.notEqual(after[0], h.pm.actorId);
    assert.deepEqual(h.core.seatActorIds(h.owner, "no-seat"), []);
    assert.throws(
      () => h.core.seatActorIds(h.developer.credential, "pm-seat"),
      /capabilit|authoriz|permit/i,
    );
  } finally {
    await close(h);
  }
});

test("a summary always fits its budget, and a body is cut by characters, not UTF-16 units", async () => {
  const h = await harness();
  try {
    for (let i = 0; i < 40; i += 1)
      h.core.enqueueMessage(ctx(h.core, h.owner), {
        recipientAgentId: h.pm.agentId,
        body: `${i} ${"x".repeat(2400)}`,
      });
    for (let i = 0; i < 150; i += 1) addWork(h, `w-${i}`, "running");
    const result = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.ok(
      Buffer.byteLength(JSON.stringify(result.summary), "utf8") <= 32 * 1024,
    );
    assert.equal(result.summary.truncated, true);
    assert.ok(result.summary.messages.length < 40);
  } finally {
    await close(h);
  }
  const g = await harness();
  try {
    // 2000 astral characters are 4000 UTF-16 units: exactly at the limit, not over it.
    g.core.enqueueMessage(ctx(g.core, g.owner), {
      recipientAgentId: g.pm.agentId,
      body: "😀".repeat(2000),
    });
    g.core.enqueueMessage(ctx(g.core, g.owner), {
      recipientAgentId: g.pm.agentId,
      body: "😀".repeat(2001),
    });
    const summary = g.core.restartAgentGeneration(
      ctx(g.core, g.owner),
      g.pm.agentId,
    ).summary;
    const [exact, over] = summary.messages;
    assert.equal(
      exact!.body,
      "😀".repeat(2000),
      "a body at the limit is kept whole",
    );
    assert.equal(over!.body, `${"😀".repeat(2000)}[truncated]`);
  } finally {
    await close(g);
  }
});

test("a very large task brief is shown as a marked preview so the summary always fits", async () => {
  const h = await harness();
  try {
    const db = writableDatabase(h);
    try {
      db.exec("DROP TRIGGER immutable_project_revisions_update");
      db.prepare(
        "UPDATE project_revisions SET content_json = ? WHERE kind = 'task_brief'",
      ).run(JSON.stringify({ text: "y".repeat(16_000) }));
    } finally {
      db.close();
    }
    const summary = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    ).summary;
    const objective = summary.objective as {
      truncated: boolean;
      rawJsonPreview: string;
    };
    assert.equal(objective.truncated, true);
    assert.ok(Array.from(objective.rawJsonPreview).length <= 2000);
  } finally {
    await close(h);
  }
});

test("a carried body that was already cut is not cut or flagged a second time", async () => {
  const h = await harness();
  try {
    h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.pm.agentId,
      body: "z".repeat(2500),
    });
    const first = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.equal(first.summary.truncated, true);
    const cut = first.summary.messages[0]!.body;
    assert.ok(cut.endsWith("[truncated]"));
    const second = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    );
    assert.equal(second.summary.messages[0]!.body, cut, "unchanged");
    assert.equal(second.summary.truncated, false, "nothing new was cut");
  } finally {
    await close(h);
  }
});

test("a fresh body that merely ends in the marker text is still cut and flagged", async () => {
  const h = await harness();
  try {
    h.core.enqueueMessage(ctx(h.core, h.owner), {
      recipientAgentId: h.pm.agentId,
      body: `${"q".repeat(2500)}[truncated]`,
    });
    const summary = h.core.restartAgentGeneration(
      ctx(h.core, h.owner),
      h.pm.agentId,
    ).summary;
    assert.equal(summary.truncated, true);
    assert.equal(Array.from(summary.messages[0]!.body).length, 2011);
  } finally {
    await close(h);
  }
});

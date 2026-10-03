import assert from "node:assert/strict";
import { test } from "node:test";
import {
  EMPTY_CHANGES,
  buildDashModel,
  historySample,
  isWorkerKind,
  pipelineItems,
  queueRows,
  trackChanges,
} from "../src/dash/model.js";
import { harness, close } from "./harness.js";
import { call } from "./harness.js";
import {
  NOW,
  degraded,
  healthy,
  iso,
  message,
  troubled,
} from "./dash-fixtures.js";

test("the header carries run state, health with its reason, supervision and workers against the limit", () => {
  const ok = buildDashModel(healthy(), NOW, 3);
  assert.deepEqual(ok.header, {
    projectId: "p1",
    runState: "active",
    runPause: null,
    supervisionEnabled: true,
    health: "healthy",
    healthReason: null,
    targetEpoch: 4,
    checkpointEpoch: 4,
    replacementAttempts: 0,
    workers: 1,
    workerLimit: 3,
  });
  const bad = buildDashModel(degraded(), NOW, null);
  assert.equal(bad.header.health, "degraded");
  assert.equal(bad.header.healthReason, "forced Supervisor evaluation failure");
  assert.equal(bad.header.workerLimit, null);
  assert.equal(buildDashModel({}, NOW, null).header.health, "unknown");
});

test("workers are active agents other than the PM and the Supervisor", () => {
  assert.equal(isWorkerKind("Developer"), true);
  assert.equal(isWorkerKind("Verifier"), true);
  assert.equal(isWorkerKind("PM"), false);
  assert.equal(isWorkerKind("Supervisor"), false);
  const status = healthy({
    agents: [
      ...(healthy().agents as object[]),
      {
        agentId: "sup",
        roleName: "supervisor",
        kind: "Supervisor",
        generation: 1,
        state: "active",
        lastActivityAt: iso(1),
      },
      {
        agentId: "old",
        roleName: "developer",
        kind: "Developer",
        generation: 1,
        state: "ended",
        lastActivityAt: iso(9000),
      },
    ],
  });
  assert.equal(buildDashModel(status, NOW, 3).header.workers, 1);
});

test("working is inferred from recent activity or a message in flight, and only for active agents", () => {
  const model = buildDashModel(healthy(), NOW, 3);
  const [pm, dev] = model.agents;
  assert.equal(pm!.working, false);
  assert.equal(dev!.working, true);
  assert.equal(model.counts.working, 1);
  const idle = healthy({
    agents: [
      {
        agentId: "developer-agent",
        roleName: "developer",
        kind: "Developer",
        generation: 1,
        state: "active",
        lastActivityAt: iso(31),
      },
    ],
  });
  assert.equal(buildDashModel(idle, NOW, 3).agents[0]!.working, false);
  const inFlight = { ...idle, messages: [message("m", "sent")] };
  assert.equal(buildDashModel(inFlight, NOW, 3).agents[0]!.working, true);
  const ended = healthy({
    agents: [
      {
        agentId: "developer-agent",
        roleName: "developer",
        kind: "Developer",
        generation: 1,
        state: "ended",
        lastActivityAt: iso(1),
      },
    ],
  });
  assert.equal(buildDashModel(ended, NOW, 3).agents[0]!.working, false);
});

test("stalled, lost and blocked flags, pane ids and queue depth come from the right fields", () => {
  const model = buildDashModel(troubled(), NOW, 3);
  const dev = model.agents.find((a) => a.agentId === "developer-agent")!;
  assert.equal(dev.stalled, true);
  assert.equal(dev.blocked, true);
  assert.equal(dev.lost, false);
  assert.equal(dev.paneId, "w1:p2");
  assert.equal(dev.queueDepth, 3);
  const pm = model.agents.find((a) => a.agentId === "pm-agent")!;
  assert.equal(pm.blocked, false);
});

test("a delivery problem is a stuck message or one that failed, expired or went unacked", () => {
  const messages = buildDashModel(troubled(), NOW, 3).queue.messages;
  const byId = Object.fromEntries(messages.map((m) => [m.messageId, m]));
  assert.equal(byId["m-stuck"]!.problem, "pane_mismatch");
  assert.equal(byId["m-failed"]!.problem, "pane_gone");
  assert.equal(byId["m-fine"]!.problem, null);
  assert.equal(byId["m-fine"]!.deferredReason, "agent_busy");
  const unacked = buildDashModel(
    healthy({ messages: [message("u", "unacked")] }),
    NOW,
    3,
  );
  assert.equal(unacked.queue.messages[0]!.problem, "unacked");
  assert.equal(byId["m-stuck"]!.recipientActive, true);
});

test("the pipeline counts each stage and lists the newest items first", () => {
  const { pipeline } = buildDashModel(troubled(), NOW, 3);
  assert.deepEqual(pipeline.reports.counts, { accepted: 1, rejected: 1 });
  assert.equal(pipeline.reports.total, 2);
  assert.deepEqual(
    pipeline.reports.items.map((i) => i.id),
    ["report:r2", "report:r1"],
  );
  assert.equal(pipeline.reports.items[0]!.label, "developer-agent fedcba9");
  assert.deepEqual(pipeline.reviews.counts, { started: 1 });
  assert.deepEqual(pipeline.integrations.counts, { merged: 1 });
  const many = healthy({
    reports: Array.from({ length: 12 }, (_, i) => ({
      reportId: `r${i}`,
      agentId: "a",
      commitSha: "0".repeat(40),
      state: "accepted",
      createdAt: iso(i),
    })),
  });
  const stage = buildDashModel(many, NOW, 3).pipeline.reports;
  assert.equal(stage.total, 12);
  assert.equal(stage.capped, false);
  assert.equal(stage.items.length, 12);
  const over = healthy({
    reports: Array.from({ length: 20 }, (_, i) => ({
      reportId: `r${i}`,
      agentId: "a",
      commitSha: "0".repeat(40),
      state: "accepted",
      createdAt: iso(i),
    })),
  });
  const capped = buildDashModel(over, NOW, 3).pipeline.reports;
  assert.equal(capped.capped, true);
  assert.equal(capped.items.length, 20);
});

test("findings show only open and escalated ones, and an escalated one needs the operator", () => {
  const { findings } = buildDashModel(troubled(), NOW, 3);
  assert.equal(findings.length, 1);
  assert.equal(findings[0]!.needsOperator, true);
  assert.equal(findings[0]!.interventions, 2);
});

test("leftovers, truncation and the v1 work rows are carried through", () => {
  const model = buildDashModel(troubled(), NOW, 3);
  assert.deepEqual(model.queue.orphanPanes, ["x-agent w1:p9"]);
  assert.deepEqual(model.queue.cleanupFailed, [
    "y-agent: worktree not removable",
  ]);
  assert.equal(
    buildDashModel(healthy({ messagesTruncated: true }), NOW, 3).queue
      .truncated,
    true,
  );
  assert.equal(buildDashModel(healthy(), NOW, 3).work.length, 0);
  const withWork = buildDashModel(
    healthy({
      work: [{ workItemId: "w1", title: "t", state: "blocked", owner: null }],
    }),
    NOW,
    3,
  );
  assert.equal(withWork.work[0]!.state, "blocked");
});

test("every ended agent is listed, most recent activity first, after the active ones", () => {
  const ended = (id: string, ago: number) => ({
    agentId: id,
    roleName: "developer",
    kind: "Developer",
    generation: 1,
    state: "ended",
    lastActivityAt: iso(ago),
  });
  const model = buildDashModel(
    healthy({
      agents: [
        ended("old", 900),
        ended("newest", 10),
        { ...ended("pm", 50), kind: "PM", state: "active" },
        ended("mid", 300),
        ...Array.from({ length: 6 }, (_, i) => ended(`bulk${i}`, 1000 + i)),
      ],
    }),
    NOW,
    3,
  );
  assert.equal(model.agents.length, 10);
  assert.deepEqual(
    model.agents.slice(0, 4).map((a) => a.agentId),
    ["pm", "newest", "mid", "old"],
  );
});

test("a review row names the reviewer and whose work it reviews", () => {
  const model = buildDashModel(
    healthy({
      reviews: [
        {
          reviewId: "v1",
          round: 2,
          state: "passed",
          authorAgentId: "developer-1",
          reviewerAgentId: "reviewer-4",
          createdAt: iso(10),
        },
        {
          reviewId: "v2",
          round: 1,
          state: "passed",
          authorAgentId: null,
          integrationId: "0123456789abcdef",
          reviewerAgentId: "reviewer-5",
          createdAt: iso(20),
        },
      ],
    }),
    NOW,
    3,
  );
  assert.deepEqual(
    model.pipeline.reviews.items.map((i) => i.label),
    ["reviewer-4 -> developer-1 r2", "reviewer-5 -> 01234567 r1"],
  );
});

test("a finding is marked when its target has ended or is not in the agent list", () => {
  const finding = (id: string, target: string, state = "escalated") => ({
    findingId: id,
    targetAgentId: target,
    severity: "medium",
    state,
    interventions: 2,
    stateReason: "kept reason",
  });
  const model = buildDashModel(
    healthy({
      agents: [
        ...(healthy().agents as unknown[]),
        {
          agentId: "gone",
          roleName: "designer",
          kind: "Designer",
          generation: 1,
          state: "ended",
          lastActivityAt: iso(100),
        },
      ],
      agentFindings: [
        finding("f1", "developer-agent"),
        finding("f2", "gone"),
        finding("f3", "gone", "open"),
        finding("f4", "nobody"),
      ],
    }),
    NOW,
    3,
  );
  assert.deepEqual(
    model.findings.map((f) => [f.findingId, f.targetState, f.stateReason]),
    [
      ["f1", "active", "kept reason"],
      ["f2", "ended", "kept reason"],
      ["f4", "unknown", "kept reason"],
      ["f3", "ended", "kept reason"],
    ],
  );
});

test("text from agents is stripped of control characters before it is shown", () => {
  const model = buildDashModel(
    healthy({
      supervisionReason: "bad\u001b[2Jreason‮",
      messages: [message("m", "failed", { stateReason: "x\ny" })],
    }),
    NOW,
    3,
  );
  assert.ok(!/[\u001b‮\n]/.test(model.header.healthReason ?? ""));
  assert.equal(model.queue.messages[0]!.stateReason, "x y");
});

test("changed rows keep their highlight for two polls and the first poll highlights nothing", () => {
  const first = buildDashModel(
    healthy({ messages: [message("m", "deferred")] }),
    NOW,
    3,
  );
  const a = trackChanges(EMPTY_CHANGES, first);
  assert.equal(a.highlight.size, 0);
  const second = buildDashModel(
    healthy({ messages: [message("m", "sent")] }),
    NOW,
    3,
  );
  const b = trackChanges(a, second);
  assert.equal(b.highlight.get("m:m"), 2);
  const c = trackChanges(b, second);
  assert.equal(c.highlight.get("m:m"), 1);
  const d = trackChanges(c, second);
  assert.equal(d.highlight.has("m:m"), false);
});

test("a status produced by a real daemon builds a model without gaps", async () => {
  const h = await harness();
  try {
    await call(h, h.owner, "send", [h.developer.agentId, "hello"]);
    const answer = await call(h, h.owner, "status");
    assert.ok(answer.ok);
    const model = buildDashModel(
      answer.result as Record<string, unknown>,
      Date.now(),
      3,
    );
    assert.equal(model.header.projectId, h.info.projectId);
    assert.equal(model.header.health, "degraded");
    assert.deepEqual(
      model.agents.map((a) => a.agentId).sort(),
      [h.developer.agentId, h.pm.agentId].sort(),
    );
    assert.equal(model.queue.messages.length, 1);
    assert.equal(
      model.queue.messages[0]!.recipientAgentId,
      h.developer.agentId,
    );
    assert.equal(model.queue.messages[0]!.recipientActive, true);
  } finally {
    await close(h);
  }
});

test("agents needing attention sort first and keep the controller order otherwise", () => {
  const model = buildDashModel(troubled(), NOW, 3);
  assert.deepEqual(
    model.agents.map((a) => a.agentId),
    ["developer-agent", "pm-agent"],
  );
  const calm = buildDashModel(healthy(), NOW, 3);
  assert.deepEqual(
    calm.agents.map((a) => a.agentId),
    ["pm-agent", "developer-agent"],
  );
});

test("delivery problems sort first in the queue and the problems filter keeps only them", () => {
  const status = healthy({
    messages: [
      message("m-ok", "queued", { sequence: 1 }),
      message("m-bad", "failed", { sequence: 2, stateReason: "pane_gone" }),
      message("m-late", "sent", { sequence: 3 }),
    ],
    stuck: [{ messageId: "m-late", reason: "no ack" }],
  });
  const model = buildDashModel(status, NOW, 3);
  assert.deepEqual(
    model.queue.messages.map((m) => m.id),
    ["m-bad", "m-late", "m-ok"],
  );
  assert.deepEqual(
    queueRows(model, true).map((m) => m.id),
    ["m-bad", "m-late"],
  );
  assert.equal(queueRows(model, false).length, 3);
});

test("escalated findings sort before open ones", () => {
  const status = healthy({
    agentFindings: [
      {
        findingId: "a",
        targetAgentId: "x",
        severity: "low",
        state: "open",
        interventions: 0,
        stateReason: null,
      },
      {
        findingId: "b",
        targetAgentId: "x",
        severity: "high",
        state: "escalated",
        interventions: 2,
        stateReason: null,
      },
    ],
  });
  assert.deepEqual(
    buildDashModel(status, NOW, 3).findings.map((f) => f.id),
    ["b", "a"],
  );
});

test("pipeline items of all stages come newest first", () => {
  const items = pipelineItems(buildDashModel(troubled(), NOW, 3));
  assert.deepEqual(
    items.map((i) => i.id),
    ["integration:i-0123456789", "report:r2", "review:v1", "report:r1"],
  );
});

test("a history sample counts unresolved and working agents and the age of the oldest message", () => {
  const model = buildDashModel(troubled(), NOW, 3);
  assert.deepEqual(historySample(model, NOW), {
    unresolved: 3,
    working: 1,
    oldestSeconds: 60,
  });
  const empty = buildDashModel(healthy(), NOW, 3);
  assert.equal(historySample(empty, NOW).oldestSeconds, 0);
  assert.equal(historySample(model, NOW + 30_000).oldestSeconds, 90);
});
